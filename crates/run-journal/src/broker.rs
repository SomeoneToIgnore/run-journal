use std::future::pending;
use std::sync::Arc;

use runvil::effect::Round;
use runvil::harness::InferenceBroker;
use runvil::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use runvil::{BoxFuture, Error};

use crate::answer;
use crate::session::{Claim, Shared};

pub(crate) struct JournalBroker {
    shared: Arc<Shared>,
    inner: Arc<dyn InferenceBroker>,
}

impl JournalBroker {
    pub(crate) fn new(shared: Arc<Shared>, inner: Arc<dyn InferenceBroker>) -> Self {
        JournalBroker { shared, inner }
    }
}

impl InferenceBroker for JournalBroker {
    fn models(&self) -> BoxFuture<'static, Result<ModelCatalog, Error>> {
        self.inner.models()
    }

    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<'static, Result<Box<Completion>, Error>> {
        let shared = Arc::clone(&self.shared);
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            match shared.claim_round(round.id.get()) {
                Claim::Stored {
                    turn,
                    payload,
                    answer,
                } => {
                    shared.turn(turn).await;
                    answer::chat(answer.as_ref(), &payload)
                }
                Claim::Dropped { turn } => {
                    shared.turn(turn).await;
                    shared.stop();
                    pending().await
                }
                Claim::Live { key } => {
                    if !shared.replayed().await {
                        pending::<()>().await;
                    }
                    let result = inner.chat(binding, messages, tools, options, round).await;
                    if let Some(key) = key {
                        shared.stash(key, answer::of_chat(&result));
                    }
                    result
                }
            }
        })
    }
}
