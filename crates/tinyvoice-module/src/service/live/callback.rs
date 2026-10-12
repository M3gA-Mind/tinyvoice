//! Tool dispatch through the host's current policy and approval context.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tinyagents_harness::{context::RunContext, ids::CallId, tool::ToolDispatch};
use tinytools::{Tool, ToolCallOptions, ToolResult};
use tinyvoice_bus::live::session::{
    CANCEL_CALLBACK_METHOD, CancelLiveToolReply, CancelLiveToolRequest, ExecuteLiveToolReply,
    ExecuteLiveToolRequest, LiveToolDeclaration, LiveToolPort, TOOL_CALLBACK_INTERFACE,
    TOOL_CALLBACK_METHOD,
};

/// Schema-only tool descriptor; typed dispatch is the only executable path.
struct Declaration(LiveToolDeclaration);

#[async_trait]
impl Tool for Declaration {
    fn name(&self) -> &str {
        &self.0.name
    }

    fn description(&self) -> &str {
        &self.0.description
    }

    fn parameters_schema(&self) -> Value {
        self.0.parameters.clone()
    }

    fn supports_markdown(&self) -> bool {
        self.0.prefer_markdown
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        anyhow::bail!("live callback execution requires an admitted provider call context")
    }
}

/// An explicitly registered tool backed by one dedicated callback method.
pub(super) struct CallbackTool {
    declaration: Arc<Declaration>,
    port: LiveToolPort,
    proxy: tinybus::Proxy,
}

impl CallbackTool {
    /// Bind a declaration to the host's saved authority without invoking it.
    pub(super) fn new(
        declaration: LiveToolDeclaration,
        port: LiveToolPort,
        connection: &tinybus::Connection,
    ) -> tinybus::Result<Self> {
        let proxy = connection.proxy(
            port.bus_name.as_str(),
            port.object_path.as_str(),
            TOOL_CALLBACK_INTERFACE,
        )?;
        Ok(Self {
            declaration: Arc::new(Declaration(declaration)),
            port,
            proxy,
        })
    }
}

#[async_trait]
impl ToolDispatch<(), ()> for CallbackTool {
    fn tool(&self) -> Arc<dyn Tool> {
        self.declaration.clone()
    }

    async fn execute(
        &self,
        _state: &(),
        call_id: CallId,
        arguments: Value,
        options: ToolCallOptions,
        _parent: &RunContext<()>,
    ) -> anyhow::Result<ToolResult> {
        let request = ExecuteLiveToolRequest {
            invocation_id: super::table::random_id()?,
            context_token: self.port.context_token.clone(),
            call_id: call_id.to_string(),
            name: self.declaration.0.name.clone(),
            arguments,
            prefer_markdown: options.prefer_markdown,
        };
        if !super::super::relay::fits(&request, 1024 * 1024) {
            anyhow::bail!("live tool callback request exceeds its serialized limit");
        }
        let mut cancellation = InvocationGuard {
            runtime: tokio::runtime::Handle::current(),
            proxy: self.proxy.clone(),
            request: Some(CancelLiveToolRequest {
                context_token: request.context_token.clone(),
                invocation_id: request.invocation_id.clone(),
                call_id: request.call_id.clone(),
            }),
        };
        let reply: ExecuteLiveToolReply = self.proxy.call(TOOL_CALLBACK_METHOD, (request,)).await?;
        cancellation.request = None;
        if !super::super::relay::fits(&reply, 1024 * 1024) {
            anyhow::bail!("live tool callback result exceeds its serialized limit");
        }
        // Preserve the complete canonical result and its reported-error flag.
        // A malformed result fails dispatch instead of synthesizing success.
        Ok(serde_json::from_value(reply.result)?)
    }
}

#[cfg(test)]
#[path = "callback_tests.rs"]
mod tests;

/// A dropped native callback cannot detach its independently owned host work.
///
/// Notification is bounded and best effort; hosts must additionally cancel
/// their invocation leases on endpoint/context teardown. Provider cancellation
/// that permits a running native tool to finish does not drop this guard early.
struct InvocationGuard {
    runtime: tokio::runtime::Handle,
    proxy: tinybus::Proxy,
    request: Option<CancelLiveToolRequest>,
}

impl Drop for InvocationGuard {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            let proxy = self.proxy.clone();
            self.runtime.spawn(async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    proxy.call::<CancelLiveToolReply>(CANCEL_CALLBACK_METHOD, (request,)),
                )
                .await;
            });
        }
    }
}
