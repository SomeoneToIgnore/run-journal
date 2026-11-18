use std::collections::BTreeMap;
use std::num::NonZeroU32;

use runvil::metrics::CallMetrics;
use runvil::model::{Completion, CompletionResult, ToolCall};
use runvil::{Error, ErrorCode, ErrorFact, OutputTrust, SourceLoc, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) fn of_chat(result: &Result<Box<Completion>, Error>) -> Option<Value> {
    let stored = match result {
        Ok(completion) => Ok(StoredCompletion::of(completion)?),
        Err(error) => Err(StoredError::of(error)),
    };
    serde_json::to_value(stored).ok()
}

pub(crate) fn chat(answer: Option<&Value>, payload: &Value) -> Result<Box<Completion>, Error> {
    let stored = answer
        .and_then(|answer| {
            serde_json::from_value::<Result<StoredCompletion, StoredError>>(answer.clone()).ok()
        })
        .or_else(|| recorded_chat(payload))
        .ok_or_else(|| unreadable("model round"))?;
    match stored {
        Ok(completion) => completion.rebuild().map(Box::new),
        Err(error) => Err(error.rebuild()),
    }
}

pub(crate) fn of_tool(result: &Result<ToolOutput, Error>) -> Option<Value> {
    let stored = match result {
        Ok(output) => Ok(StoredOutput {
            text: output.text().to_owned(),
            trust: output.trust(),
        }),
        Err(error) => Err(StoredError::of(error)),
    };
    serde_json::to_value(stored).ok()
}

pub(crate) fn tool(answer: Option<&Value>, payload: &Value) -> Result<ToolOutput, Error> {
    let stored = answer
        .and_then(|answer| {
            serde_json::from_value::<Result<StoredOutput, StoredError>>(answer.clone()).ok()
        })
        .or_else(|| recorded_tool(payload))
        .ok_or_else(|| unreadable("tool call"))?;
    match stored {
        Ok(output) => Ok(output.rebuild()),
        Err(error) => Err(error.rebuild()),
    }
}

pub(crate) fn agrees(answer: &Value, payload: &Value) -> bool {
    let recorded = payload.get("ToolCall");
    match serde_json::from_value::<Result<StoredOutput, StoredError>>(answer.clone()) {
        Ok(Ok(output)) => {
            recorded.and_then(|recorded| recorded.get("Ok"))
                == serde_json::to_value(&output).ok().as_ref()
        }
        Ok(Err(error)) => recorded
            .and_then(|recorded| recorded.get("Err"))
            .is_some_and(|failure| {
                failure.get("kind").and_then(Value::as_str) == Some(error.code.as_str())
                    && failure.get("message").and_then(Value::as_str) == Some(error.prose.as_str())
            }),
        Err(_) => false,
    }
}

#[derive(Serialize, Deserialize)]
struct StoredCompletion {
    model: String,
    finish_reason: Option<String>,
    reasoning_content: Option<String>,
    reply: Option<String>,
    tool_calls: Vec<StoredCall>,
    metrics: Option<CallMetrics>,
    metadata_diagnostics: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredCall {
    id: String,
    name: String,
    arguments: Value,
}

#[derive(Serialize, Deserialize)]
struct StoredOutput {
    text: String,
    trust: OutputTrust,
}

#[derive(Serialize, Deserialize)]
struct StoredError {
    code: String,
    message: String,
    prose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<StoredLoc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    facts: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct StoredLoc {
    file: String,
    start_line: NonZeroU32,
    start_column: NonZeroU32,
    end_line: NonZeroU32,
    end_column: NonZeroU32,
}

impl StoredCompletion {
    fn of(completion: &Completion) -> Option<StoredCompletion> {
        let (reply, tool_calls) = match completion.result() {
            CompletionResult::Text(text) => (Some(text.clone()), Vec::new()),
            CompletionResult::ToolCalls(calls) => (
                None,
                calls
                    .iter()
                    .map(StoredCall::of)
                    .collect::<Option<Vec<_>>>()?,
            ),
            _ => return None,
        };
        Some(StoredCompletion {
            model: completion.model().to_owned(),
            finish_reason: completion.finish_reason().map(str::to_owned),
            reasoning_content: completion.reasoning_content().map(str::to_owned),
            reply,
            tool_calls,
            metrics: completion.metrics().cloned(),
            metadata_diagnostics: completion.metadata_diagnostics().to_vec(),
        })
    }

    fn rebuild(self) -> Result<Completion, Error> {
        let result = match self.reply {
            Some(reply) if self.tool_calls.is_empty() => CompletionResult::Text(reply),
            _ => CompletionResult::ToolCalls(
                self.tool_calls
                    .into_iter()
                    .map(|call| ToolCall::from_parts(call.id, call.name, call.arguments))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        let mut completion = Completion::from_result(result, self.model)?
            .with_metadata_diagnostics(self.metadata_diagnostics);
        if let Some(finish_reason) = self.finish_reason {
            completion = completion.with_finish_reason(finish_reason);
        }
        if let Some(reasoning_content) = self.reasoning_content {
            completion = completion.with_reasoning_content(reasoning_content);
        }
        if let Some(metrics) = self.metrics {
            completion = completion.with_metrics(metrics);
        }
        Ok(completion)
    }
}

impl StoredCall {
    fn of(call: &ToolCall) -> Option<StoredCall> {
        Some(StoredCall {
            id: call.id().to_owned(),
            name: call.name().to_owned(),
            arguments: serde_json::from_str::<Value>(&call.arguments().to_json_string()).ok()?,
        })
    }
}

impl StoredOutput {
    fn rebuild(self) -> ToolOutput {
        if self.trust == OutputTrust::Trusted {
            ToolOutput::trusted(self.text)
        } else {
            ToolOutput::untrusted(self.text)
        }
    }
}

impl StoredError {
    fn of(error: &Error) -> StoredError {
        StoredError {
            code: error.code.tag().to_owned(),
            message: error.message.clone(),
            prose: error.prose.clone(),
            at: error.at.as_ref().map(|at| StoredLoc {
                file: at.file.clone(),
                start_line: at.start_line,
                start_column: at.start_column,
                end_line: at.end_line,
                end_column: at.end_column,
            }),
            facts: error
                .facts
                .iter()
                .map(|(fact, value)| (format!("{fact:?}"), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn rebuild(self) -> Error {
        let code = ErrorCode::from_tag(&self.code).unwrap_or(ErrorCode::Internal);
        let mut error = Error::new(code, self.message, self.prose);
        error.at = self.at.and_then(|at| {
            SourceLoc::new(
                at.file,
                at.start_line,
                at.start_column,
                at.end_line,
                at.end_column,
            )
        });
        error.facts = self
            .facts
            .into_iter()
            .filter_map(|(name, value)| Some((fact(&name)?, value)))
            .collect::<BTreeMap<_, _>>();
        error
    }
}

const FACTS: [ErrorFact; 14] = [
    ErrorFact::Path,
    ErrorFact::Name,
    ErrorFact::Task,
    ErrorFact::Tasks,
    ErrorFact::Anchor,
    ErrorFact::Count,
    ErrorFact::Limit,
    ErrorFact::Rule,
    ErrorFact::ValidUpTo,
    ErrorFact::Reason,
    ErrorFact::FinishReason,
    ErrorFact::Section,
    ErrorFact::Chain,
    ErrorFact::Run,
];

fn fact(name: &str) -> Option<ErrorFact> {
    FACTS.into_iter().find(|fact| format!("{fact:?}") == name)
}

fn recorded_chat(payload: &Value) -> Option<Result<StoredCompletion, StoredError>> {
    let recorded = payload.get("Chat")?;
    if let Some(failure) = recorded.get("Err") {
        return Some(Err(recorded_failure(failure)?));
    }
    let completion = recorded.get("Ok")?;
    let calls = completion.get("tool_calls")?.as_array()?;
    if !calls.is_empty() {
        return None;
    }
    Some(Ok(StoredCompletion {
        model: completion.get("model")?.as_str()?.to_owned(),
        finish_reason: completion
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reasoning_content: None,
        reply: Some(completion.get("reply")?.as_str()?.to_owned()),
        tool_calls: Vec::new(),
        metrics: None,
        metadata_diagnostics: Vec::new(),
    }))
}

fn recorded_tool(payload: &Value) -> Option<Result<StoredOutput, StoredError>> {
    let recorded = payload.get("ToolCall")?;
    if let Some(failure) = recorded.get("Err") {
        return Some(Err(recorded_failure(failure)?));
    }
    serde_json::from_value::<StoredOutput>(recorded.get("Ok")?.clone())
        .ok()
        .map(Ok)
}

fn recorded_failure(failure: &Value) -> Option<StoredError> {
    let message = failure.get("message")?.as_str()?;
    Some(StoredError {
        code: failure.get("kind")?.as_str()?.to_owned(),
        message: message.to_owned(),
        prose: message.to_owned(),
        at: None,
        facts: BTreeMap::new(),
    })
}

fn unreadable(what: &str) -> Error {
    let message = format!("the journal holds no readable answer for this {what}");
    Error::new(ErrorCode::Internal, message.clone(), message)
}
