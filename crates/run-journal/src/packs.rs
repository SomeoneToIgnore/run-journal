use std::sync::Arc;

use runvil::packs::PackId;
use runvil::plugin::{Pack, PackFactory, PackOrigin, PackRegistry, RenderContext, ToolContext};
use runvil::{BoxFuture, Error, ToolInfo, ToolOutput};
use serde_json::Value;

use crate::answer;
use crate::session::{Rendered, Shared};

pub(crate) fn wrap(shared: &Arc<Shared>, registry: PackRegistry) -> Result<PackRegistry, Error> {
    let builtin = registry
        .list()
        .filter(|(_, origin)| *origin == PackOrigin::Builtin)
        .map(|(info, _)| info.fqn)
        .collect::<Vec<_>>();
    let mut wrapped = PackRegistry::new();
    for (info, _, factory) in registry.into_entries() {
        if builtin.contains(&info.fqn) {
            continue;
        }
        match factory {
            Ok(inner) => wrapped.insert(
                info,
                JournalFactory {
                    shared: Arc::clone(shared),
                    inner,
                },
            )?,
            Err(reason) => wrapped.insert_unavailable(info, reason)?,
        }
    }
    Ok(wrapped)
}

struct JournalFactory {
    shared: Arc<Shared>,
    inner: Arc<dyn PackFactory>,
}

impl PackFactory for JournalFactory {
    fn new_pack(&self, id: &PackId) -> Result<Arc<dyn Pack>, Error> {
        Ok(Arc::new(JournalPack {
            shared: Arc::clone(&self.shared),
            id: serde_json::to_value(id).unwrap_or_default(),
            inner: self.inner.new_pack(id)?,
        }))
    }
}

struct JournalPack {
    shared: Arc<Shared>,
    id: Value,
    inner: Arc<dyn Pack>,
}

impl Pack for JournalPack {
    fn tools(&self) -> Vec<ToolInfo> {
        self.inner.tools()
    }

    fn ready(&self) -> BoxFuture<'_, Result<(), Error>> {
        self.inner.ready()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        args: Value,
    ) -> BoxFuture<'a, Result<ToolOutput, Error>> {
        Box::pin(async move {
            let call = Value::Array(Vec::from([
                serde_json::to_value(cx.tool()).unwrap_or_default(),
                args.clone(),
                serde_json::to_value(cx.origin()).unwrap_or_default(),
            ]));
            let claim = self.shared.claim_call(&call);
            let live = || self.inner.call(cx, args);
            self.shared
                .answer(claim, answer::tool, live, answer::of_tool)
                .await
        })
    }

    fn render<'a>(
        &'a self,
        cx: RenderContext<'a>,
        offered: Vec<ToolInfo>,
    ) -> BoxFuture<'a, Vec<ToolInfo>> {
        Box::pin(async move {
            match self.shared.claim_render(cx.entry, &self.id) {
                Rendered::Stored { turn, tools } => {
                    self.shared.turn(turn).await;
                    tools
                }
                Rendered::Unreadable { turn } => {
                    self.shared.turn(turn).await;
                    self.inner.render(cx, offered).await
                }
                Rendered::Live => {
                    self.shared.live().await;
                    self.inner.render(cx, offered).await
                }
            }
        })
    }
}
