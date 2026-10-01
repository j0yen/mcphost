//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC6 (P1) — Given `host.quickstart kind=python` (or `docs/kinds/python.md`
//! when the discoverability PRD is absent), When read, Then `mcphost.channel`
//! and `mcphost.msg` appear with the signatures in requirement 1. The
//! sandbox-bridge-discoverability PRD (`sandbox_api.modules`) has not
//! landed (no `sandbox_api` anywhere in `src/`), so `docs/kinds/python.md`
//! is this AC's documented surface.

const PYTHON_KIND_DOC: &str = include_str!("../docs/kinds/python.md");

#[test]
fn python_kind_doc_documents_channel_and_msg_with_their_signatures() {
    assert!(
        PYTHON_KIND_DOC.contains("mcphost.channel"),
        "docs/kinds/python.md must mention mcphost.channel"
    );
    assert!(
        PYTHON_KIND_DOC.contains("mcphost.msg"),
        "docs/kinds/python.md must mention mcphost.msg"
    );
    assert!(
        PYTHON_KIND_DOC.contains("channel.post(channel_id, body, *, kind=None)"),
        "docs/kinds/python.md must document channel.post's signature"
    );
    assert!(
        PYTHON_KIND_DOC.contains("channel.read(channel_id, cursor=None, limit=100, ack=False)"),
        "docs/kinds/python.md must document channel.read's signature"
    );
    assert!(
        PYTHON_KIND_DOC.contains("msg.send(to, body, *, thread=None, urgent=False)"),
        "docs/kinds/python.md must document msg.send's signature"
    );
    assert!(
        PYTHON_KIND_DOC.contains("msg.inbox(limit=50, cursor=None)"),
        "docs/kinds/python.md must document msg.inbox's signature"
    );
}
