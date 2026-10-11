//! Native bus fixtures for exact tool provenance and current host authority.

use super::*;
use serde_json::json;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tinyagents_harness::context::RunConfig;
use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;

#[derive(Clone, Default)]
struct Authority {
    revoked: Arc<AtomicBool>,
    malformed: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<ExecuteLiveToolRequest>>>,
}

#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.LiveTools")]
impl Authority {
    async fn execute_live_tool(
        &self,
        request: ExecuteLiveToolRequest,
    ) -> tinybus::Result<ExecuteLiveToolReply> {
        if request.context_token != "known-context" || request.name != "approved" {
            return Err(tinybus::Error::failed("unknown host authority"));
        }
        self.seen
            .lock()
            .map_err(|_| tinybus::Error::failed("fixture lock"))?
            .push(request);
        if self.malformed.load(Ordering::SeqCst) {
            return Ok(ExecuteLiveToolReply {
                result: json!({"invented_success": true}),
            });
        }
        let mut result = if self.revoked.load(Ordering::SeqCst) {
            ToolResult::error("revoked by current host policy")
        } else {
            ToolResult::success("approved result")
        };
        result.metadata = Some(json!({"host_only": "retained"}));
        result.markdown_formatted = Some("rendered".into());
        Ok(ExecuteLiveToolReply {
            result: serde_json::to_value(result)
                .map_err(|error| tinybus::Error::failed(error.to_string()))?,
        })
    }
}

async fn fixture() -> tinybus::Result<(CallbackTool, Authority, tinybus::Connection)> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let authority = Authority::default();
    let host = tinybus::Connection::connect(bus.connect().await?).await?;
    host.serve_at("/test/LiveTools".try_into()?, authority.clone())
        .await?;
    host.request_name("ai.test.LiveTools").await?;
    let module = tinybus::Connection::connect(bus.connect().await?).await?;
    let tool = CallbackTool::new(
        LiveToolDeclaration {
            name: "approved".into(),
            description: "approved declaration".into(),
            parameters: json!({"type": "object"}),
            prefer_markdown: true,
        },
        LiveToolPort {
            bus_name: "ai.test.LiveTools".into(),
            object_path: "/test/LiveTools".into(),
            context_token: "known-context".into(),
        },
        &module,
    )?;
    Ok((tool, authority, host))
}

fn context() -> RunContext<()> {
    RunContext::new(RunConfig::new("fixture"), ())
}

#[tokio::test]
async fn callback_preserves_native_call_identity_arguments_and_complete_host_result()
-> anyhow::Result<()> {
    let (tool, authority, _host) = fixture().await?;
    let result = tool
        .execute(
            &(),
            CallId::new("provider-call-7"),
            json!({"argument": "value"}),
            ToolCallOptions {
                prefer_markdown: true,
            },
            &context(),
        )
        .await?;
    assert!(!result.is_error);
    assert_eq!(result.markdown_formatted.as_deref(), Some("rendered"));
    assert_eq!(result.metadata, Some(json!({"host_only": "retained"})));
    let seen = authority
        .seen
        .lock()
        .map_err(|_| anyhow::anyhow!("fixture lock"))?;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].call_id, "provider-call-7");
    assert_eq!(seen[0].arguments, json!({"argument": "value"}));
    assert!(seen[0].prefer_markdown);
    Ok(())
}

#[tokio::test]
async fn saved_declaration_never_overrides_revoked_live_host_policy() -> anyhow::Result<()> {
    let (tool, authority, _host) = fixture().await?;
    authority.revoked.store(true, Ordering::SeqCst);
    let result = tool
        .execute(
            &(),
            CallId::new("revoked-call"),
            json!({}),
            ToolCallOptions::default(),
            &context(),
        )
        .await?;
    assert!(result.is_error);
    assert_eq!(result.text(), "revoked by current host policy");
    assert!(tool.tool().execute(json!({})).await.is_err());
    Ok(())
}

#[tokio::test]
async fn malformed_host_result_is_not_fabricated_as_success() -> anyhow::Result<()> {
    let (tool, authority, _host) = fixture().await?;
    authority.malformed.store(true, Ordering::SeqCst);
    assert!(
        tool.execute(
            &(),
            CallId::new("bad-reply"),
            json!({}),
            ToolCallOptions::default(),
            &context()
        )
        .await
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn oversized_tool_arguments_fail_before_calling_host_executor() -> anyhow::Result<()> {
    let (tool, authority, _host) = fixture().await?;
    assert!(
        tool.execute(
            &(),
            CallId::new("oversized"),
            json!({"payload": "x".repeat(1024 * 1024)}),
            ToolCallOptions::default(),
            &context()
        )
        .await
        .is_err()
    );
    assert!(
        authority
            .seen
            .lock()
            .map_err(|_| anyhow::anyhow!("fixture lock"))?
            .is_empty()
    );
    Ok(())
}
