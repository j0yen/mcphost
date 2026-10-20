//! One function per dotted host verb: the arm bodies `dispatch_tenant_tool_inner`
//! used to hold in a single `match name`, moved verbatim
//! (PRD-mcphost-generated-dispatch-table-stack-budget R3). Each returns its future
//! already pinned on the heap, so the dispatcher's poll frame holds a pointer and
//! not 222 callee futures. `src/dispatch_table.rs` (GENERATED from the descriptor
//! registry by `scripts/gen-dispatch-table.sh`) names every function here; a row
//! without a function fails `cargo check`, a function without a row fails
//! `tests/fwdtab_ac02_*`.

use super::*;

/// What one arm needs from `dispatch_tenant_tool_inner`'s arguments.
pub struct ArmCtx<'a> {
    pub tenant: &'a Tenant,
    pub subject: Option<&'a str>,
    pub auth_method: &'a str,
    /// `calls.auth_method`'s own `{key, issuer_jwt, hosted_token}` domain (see
    /// `dispatch_tenant_tool_inner`).
    pub calls_auth_method: &'a str,
    pub end_user: Option<&'a crate::enduser::EndUser>,
    pub session_id: Option<&'a str>,
    pub name: &'a str,
    pub args: Value,
}

/// An arm's future, built on the heap.
pub type ArmFut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, AppError>> + Send + 'a>>;

/// `Box::pin` that also pins the future's output type, so an arm body can use `?`.
fn box_arm<'a, F>(fut: F) -> ArmFut<'a>
where
    F: std::future::Future<Output = Result<Value, AppError>> + Send + 'a,
{
    Box::pin(fut)
}

impl McpHostHandler {
    /// Verbs the registry advertises that `call_tool_inner` answers BEFORE
    /// dispatch (anonymous-capable: quickstart, help, errors, feedback, redeem). A
    /// call that reaches the table for one of them is what it was when the match
    /// had no arm: `unknown_host_tool`.
    fn answered_before_dispatch<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move { Err(unknown_host_tool(&self.state.kinds, c.name)) })
    }

    /// Verbs `dispatch_tenant_tool_inner` serves that are not host descriptors in
    /// the registry's canonical dotted set: `billing.*` and the conditionally
    /// listed `host.tool_from_description`.
    pub(crate) fn arm_unadvertised<'a>(&'a self, c: ArmCtx<'a>) -> Option<ArmFut<'a>> {
        let name = c.name;
        match name {
            "host.tool_from_description" => Some(box_arm(async move { let ArmCtx { tenant, args, .. } = c; self.tool_from_description(tenant, args).await })),
            "billing.status" => Some(box_arm(async move { let ArmCtx { tenant, .. } = c; crate::billing::status(&self.state, tenant).await })),
            "billing.checkout" => Some(box_arm(async move { let ArmCtx { tenant, args, .. } = c; crate::billing::checkout(&self.state, tenant, &args).await })),
            _ => None,
        }
    }

    pub(crate) fn arm_host_errors<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_feedback_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_feedback_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_feedback_send<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_help_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_help_search<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_quickstart<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }
    pub(crate) fn arm_host_redeem<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> { self.answered_before_dispatch(c) }

    pub(crate) fn arm_host_whoami<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, subject, auth_method, session_id, .. } = c;
            {
                let session_tenant = self.session_tenant_namespace(session_id).await;
                let mut me =
                    control::whoami(&self.state, tenant, subject, auth_method, session_tenant.as_deref()).await?;
                let siblings = crate::tenant_clients::whoami_fields(
                    &self.state,
                    tenant,
                    crate::tenant_clients::current_client().as_deref(),
                )
                .await?;
                if let (Some(obj), Some(extra)) = (me.as_object_mut(), siblings.as_object()) {
                    for (k, v) in extra {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                if let (Some(mut key), Some(obj)) =
                    (crate::subkeys::whoami_key(&self.state, session_id).await?, me.as_object_mut())
                {
                    // PRD-mcphost-delegation-recipes-and-quickstart R6: the
                    // helper hint rides top-level, not inside `key`.
                    if let Some(hint) = key.as_object_mut().and_then(|k| k.remove("hint")) {
                        obj.insert("hint".to_string(), hint);
                    }
                    obj.insert("key".to_string(), key);
                }
                // PRD-mcphost-scope-registry-generated R5: the registry and
                // the caller's own effective set.
                let call_scope = crate::scopes::CALL_SCOPE.try_with(|s| s.clone()).ok().flatten();
                let catalog: Vec<String> = self
                    .state
                    .db
                    .list_oauth_scopes(tenant.id)
                    .await?
                    .into_iter()
                    .map(|r| r.name)
                    .collect();
                if let Some(obj) = me.as_object_mut() {
                    obj.insert("scopes".to_string(), crate::scopes::whoami_json(call_scope.as_deref(), &catalog));
                }
                Ok(me)
            }
        })
    }

    pub(crate) fn arm_host_key_rotate<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            {
                // PRD-mcphost-same-user-second-agent R5: a sibling rotates
                // only its own secret; `all: true` rotates every sibling and
                // the join code. Client 0 alone keeps today's shape.
                let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
                let first = crate::tenant_clients::first_client_id(tenant.id);
                let client = crate::tenant_clients::current_client().unwrap_or_else(|| first.clone());
                if all || client != first {
                    crate::tenant_clients::rotate(&self.state, tenant, &client, all).await
                } else {
                    control::key_rotate(&self.state, tenant).await
                }
            }
        })
    }

    pub(crate) fn arm_host_client_revoke<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tenant_clients::revoke(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-connection-provider-registry: `host.connect.providers`.
    pub(crate) fn arm_host_connect_providers<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { args, .. } = c;
            crate::connect::providers_tool(&self.state, &args)
        })
    }

    pub(crate) fn arm_host_calls_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::calllog::list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_calls_export<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::calllog::export(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_key_mint<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::subkeys::mint(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_key_delegate<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, session_id, args, .. } = c;
            crate::subkeys::delegate(&self.state, tenant, &args, session_id).await
        })
    }

    pub(crate) fn arm_host_key_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::subkeys::list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_key_revoke<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::subkeys::revoke(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_self_offboard<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, session_id, .. } = c;
            control::self_offboard(&self.state, tenant, session_id).await
        })
    }

    pub(crate) fn arm_host_tool_publish<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.tool_publish_with_example(tenant, args).await
        })
    }

    pub(crate) fn arm_host_tool_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::tool_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::tool_remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_logs<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::tool_logs(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_bridge_test<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.bridge_test(tenant, args).await
        })
    }

    pub(crate) fn arm_host_spec_test<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.spec_test(tenant, args).await
        })
    }

    pub(crate) fn arm_host_tool_run<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.tool_run(tenant, args).await
        })
    }

    // The forwarding wrappers: also reached, ahead of this match, by
    // `dispatch_tenant_tool` (see `FORWARDING_WRAPPERS`).
    pub(crate) fn arm_host_tool_call<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, calls_auth_method, end_user, name, args, .. } = c;
            {
                self.forwarding_wrapper(tenant, calls_auth_method, end_user, name, args).await
            }
        })
    }

    // The forwarding wrappers: also reached, ahead of this match, by
    // `dispatch_tenant_tool` (see `FORWARDING_WRAPPERS`).
    pub(crate) fn arm_host_tool_test<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, calls_auth_method, end_user, name, args, .. } = c;
            {
                self.forwarding_wrapper(tenant, calls_auth_method, end_user, name, args).await
            }
        })
    }

    // The forwarding wrappers: also reached, ahead of this match, by
    // `dispatch_tenant_tool` (see `FORWARDING_WRAPPERS`).
    pub(crate) fn arm_host_estimate<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, calls_auth_method, end_user, name, args, .. } = c;
            {
                self.forwarding_wrapper(tenant, calls_auth_method, end_user, name, args).await
            }
        })
    }

    pub(crate) fn arm_host_uptime_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, calls_auth_method, end_user, args, .. } = c;
            self.uptime_create(tenant, args, calls_auth_method, end_user).await
        })
    }

    pub(crate) fn arm_host_call_trace<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.call_trace(Some(tenant), args).await
        })
    }

    pub(crate) fn arm_host_doctor<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.doctor(Some(tenant), args).await
        })
    }

    pub(crate) fn arm_host_webhook_inbox_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::webhook_inbox::create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_history<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::tool_history(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_rollback<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::tool_rollback(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_diff<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::tool_diff(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_usage<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::usage(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_changelog<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::changelog(&self.state, tenant, &args)
        })
    }

    pub(crate) fn arm_host_resume<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::resume::resume(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_export<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::export::export(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_import<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::import::import(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_share<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.tool_share_tested(tenant, args).await
        })
    }

    pub(crate) fn arm_host_tool_spec_shared<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.tool_spec_shared(tenant, args).await
        })
    }

    pub(crate) fn arm_host_tool_list_shared<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::list_shared(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_fork<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::tool_fork(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_tool_unshare<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::tool_unshare(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_share_caller_limit<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            {
                crate::sharing::caller_limit(&self.state, tenant, &args).await
            }
        })
    }

    pub(crate) fn arm_host_share_caller_limit_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            {
                crate::sharing::caller_limit_remove(&self.state, tenant, &args).await
            }
        })
    }

    pub(crate) fn arm_host_group_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::group_create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_group_add<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::group_add(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_group_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::group_remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_group_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::sharing::group_list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_catalog_search<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { args, .. } = c;
            crate::sharing::catalog_search(&self.state, &args).await
        })
    }

    pub(crate) fn arm_host_catalog_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::sharing::catalog_get(&self.state, Some(tenant), &args).await
        })
    }

    pub(crate) fn arm_host_invite_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::invites::create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_invite_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::invites::list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_invite_revoke<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::invites::revoke(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org::create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_join<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org::join(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_members<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org::members(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org::remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_leave<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org::leave(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_digest_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org_digest::set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_digest_send<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org_digest::send(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_org_usage<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::org_usage::usage(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-send-email-from-tool.
    pub(crate) fn arm_host_email_send<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::email_send::send(&self.state, tenant, None, &args).await
        })
    }

    pub(crate) fn arm_host_email_sends<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::email_send::sends(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_secret_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::secret_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_secret_leak_test<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, calls_auth_method, end_user, .. } = c;
            self.secret_leak_test(tenant, args, calls_auth_method, end_user).await
        })
    }

    pub(crate) fn arm_host_secret_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            control::secret_list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_registry_publish<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            control::registry_publish(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_state_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            tenant_state::state_get(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_state_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            tenant_state::state_set(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_state_delete<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                tenant_state::state_delete(&self.state, tenant, &args, end_user).await
            }
        })
    }

    pub(crate) fn arm_host_state_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            tenant_state::state_list(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_note_put<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notes::put(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_note_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notes::list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_note_delete<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notes::delete(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_state_table_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            {
                tenant_state::state_table_create(&self.state, tenant, &args).await
            }
        })
    }

    pub(crate) fn arm_host_state_table_drop<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            {
                tenant_state::state_table_drop(&self.state, tenant, &args).await
            }
        })
    }

    pub(crate) fn arm_host_state_insert<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                tenant_state::state_insert(&self.state, tenant, &args, end_user).await
            }
        })
    }

    pub(crate) fn arm_host_state_query<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                tenant_state::state_query(&self.state, tenant, &args, end_user).await
            }
        })
    }

    pub(crate) fn arm_host_state_delete_rows<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                tenant_state::state_delete_rows(&self.state, tenant, &args, end_user).await
            }
        })
    }

    // PRD-mcphost-row-policy requirement 7: tenant-key only.
    pub(crate) fn arm_host_policy_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            crate::rowpolicy::policy_set(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_policy_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, .. } = c;
            crate::rowpolicy::policy_list(&self.state, tenant, end_user).await
        })
    }

    pub(crate) fn arm_host_policy_attrs_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                crate::rowpolicy::policy_attrs_set(&self.state, tenant, &args, end_user).await
            }
        })
    }

    pub(crate) fn arm_host_audit_chain<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            crate::rowpolicy::audit_chain(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_audit_export<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::audit_export::export(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_audit_verify<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            crate::rowpolicy::audit_verify(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_enduser_whoami<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { end_user, .. } = c;
            Ok(crate::enduser::whoami(end_user))
        })
    }

    pub(crate) fn arm_host_enduser_assertion_secret_rotate<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            {
                crate::enduser::assertion_secret_rotate(&self.state, tenant, &args).await
            }
        })
    }

    pub(crate) fn arm_host_vault_provider_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::vault::provider_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_vault_providers<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::vault::providers(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_vault_connect_link<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                crate::vault::connect_link(&self.state, tenant, &args, end_user).await
            }
        })
    }

    pub(crate) fn arm_host_vault_disconnect<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            crate::vault::disconnect(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_vault_status<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            crate::vault::status(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_vault_provider_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::vault::provider_remove(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-owner-verification-paths R1: one arm per `host.claim.*` tool.
    pub(crate) fn arm_host_claim_start<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::verify::claim_start(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_claim_github<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::verify::claim_github(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_claim_dns_start<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::verify::claim_dns(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_claim_dns_check<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::verify::claim_dns_check(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-end-user-audit-and-revoke requirement 2 (AC1/AC10).
    pub(crate) fn arm_host_enduser_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::enduserctl::list(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-end-user-audit-and-revoke requirement 2 (AC2).
    pub(crate) fn arm_host_enduser_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::enduserctl::get(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-end-user-audit-and-revoke requirement 3 (AC3/AC7).
    pub(crate) fn arm_host_enduser_audit<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::enduserctl::audit(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-end-user-audit-and-revoke requirement 4 (AC4/AC7).
    pub(crate) fn arm_host_enduser_revoke<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::enduserctl::revoke(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_enduser_unrevoke<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::enduserctl::unrevoke(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-end-user-audit-and-revoke requirement 5 (AC5/AC6).
    pub(crate) fn arm_host_enduser_purge<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::enduserctl::purge(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-end-user-audit-and-revoke requirement 6 (AC8).
    pub(crate) fn arm_host_enduser_export<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::export::enduser_export(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_append<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_append(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_query<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            tables::table_query(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_table_query_diagnose<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_query_diagnose(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_query_stats<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_query_stats(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_drop<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_drop(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_lineage_blast_radius<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::lineage::blast_radius(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_lineage_trace<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::lineage::trace(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_lineage_trace_page<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::lineage::trace_page(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_schema<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_schema(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_chart<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            {
                crate::chart::table_chart(&self.state, tenant, &args, end_user).await
            }
        })
    }

    pub(crate) fn arm_host_table_charts<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::chart::table_charts_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_query_log<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_query_log(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_count<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_count(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_handles<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_handles(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_handle_drop<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            tables::table_handle_drop(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_handle_export<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::export::handle_export(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_put<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_put(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_put<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_put(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_get(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_delete<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_delete(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_url<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_url(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_touch<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_touch(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_policy<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_policy(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_blob_gc<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::blobs::blob_gc(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_get(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_delete<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_delete(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_status<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_status(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_purge<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_purge(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_describe<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tables_model::table_describe(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_model_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tables_model::model_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_models<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tables_model::table_models_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_graph<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tables_graph::table_graph(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_join_paths<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tables_graph::join_paths(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_table_next_questions<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tables_graph::next_questions(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_search<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, end_user, args, .. } = c;
            docs::doc_search(&self.state, tenant, &args, end_user).await
        })
    }

    pub(crate) fn arm_host_docs_index_config<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_index_config(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_docs_reindex<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            docs::doc_reindex(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_index_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::index::index_create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_index_status<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::index::index_status(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_index_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::index::index_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_index_delete<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::index::index_delete(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_index_query<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::index::index_query(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_index_refresh<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::index::index_refresh(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_drift_reviews<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::drift::reviews(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_drift_review<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::drift::review_item(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_drift_resolve<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::drift::resolve(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_drift_check<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::drift::check(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_runs_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::get(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_runs_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_runs_cancel<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::cancel(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_runs_purge<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::purge(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_runs_wait<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::wait(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_runs_part<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::part(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_job_timeline<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::timeline::job_timeline(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_progress<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::runs::progress(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::get(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_pause<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::pause(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_resume<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::resume(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_fire<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::triggers::fire(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_schedule_receipts<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::receipts::schedule_receipts(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_test<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::hooks::test(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_replay<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::hooks::replay(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_trigger_dead_letters<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::deadletters::list(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-agent-directory requirements 2-5.
    pub(crate) fn arm_host_agent_whoami<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            agents::whoami(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_agent_profile_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            agents::profile_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_lookup<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { args, .. } = c;
            agents::lookup(&self.state, &args).await
        })
    }

    pub(crate) fn arm_host_agent_search<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { args, .. } = c;
            agents::search(&self.state, &args).await
        })
    }

    // PRD-mcphost-agent-inbox requirements 2-5, 9.
    pub(crate) fn arm_host_example_propose<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            self.example_propose(tenant, args).await
        })
    }

    pub(crate) fn arm_host_example_proposal_status<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::example_propose::proposal_status(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_example_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { args, .. } = c;
            crate::example_proofs::list(&self.state, &args).await
        })
    }

    pub(crate) fn arm_host_example_fork<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { args, .. } = c;
            crate::example_proofs::fork(&self.state, &args).await
        })
    }

    pub(crate) fn arm_host_msg_send<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::send(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_reply<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::reply(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_inbox<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::inbox(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_wait<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::wait(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_thread<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::thread(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_ack<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::ack(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-ask-user-decision.
    pub(crate) fn arm_host_ask_create<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::asks::create(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_ask_wait<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::asks::wait(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_ask_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::asks::get(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_ask_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::asks::list(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-notify-user-channel.
    pub(crate) fn arm_host_notify_channel_add<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notify::channel_add(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_notify_channel_list<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notify::channel_list(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_notify_channel_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notify::channel_remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_notify_send<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notify::send(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_notify_receipts<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::notify::receipts(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_task_post<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tasks::post(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_task_claim<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tasks::claim(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_task_complete<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tasks::complete(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_task_get<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tasks::get(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_task_dead_letters<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::tasks::dead_letters(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_block<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::block(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_msg_unblock<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            messaging::unblock(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-agent-consent requirements 2, 3, 5, 10.
    pub(crate) fn arm_host_agent_contact_request<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::contact_request(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_contacts<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::contacts(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_contact_accept<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::contact_accept(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_contact_deny<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::contact_deny(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_mute<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::mute(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_unmute<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::unmute(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_agent_contacts_import<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            consent::contacts_import(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-agent-mesh-ops: the minimal `host.channel.*` slice.
    pub(crate) fn arm_host_channel_open<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            channels::open(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_channel_post<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            channels::post(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_channel_read<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            channels::read(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_channel_close<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            channels::close(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_channel_freeze<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            channels::freeze(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_channel_unfreeze<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            channels::unfreeze(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-oauth-resource-server requirement 3.
    pub(crate) fn arm_host_oauth_issuer_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth::issuer_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_issuer_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth::issuer_remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_issuers<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::oauth::issuers_list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_scope_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth::scope_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_scopes<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::oauth::scopes_list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_grants<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::authz::grants_list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_grant_revoke<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::authz::grant_revoke(&self.state, tenant, &args).await
        })
    }

    // PRD-mcphost-oauth-client-policy requirements 1-4 (AC1-AC6).
    pub(crate) fn arm_host_oauth_policy_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth_policy::policy_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_policy<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::oauth_policy::policy_get(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_pending<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::oauth_policy::pending_list(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_client_approve<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth_policy::client_approve(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_client_deny<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth_policy::client_deny(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_revoke_all<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::oauth_policy::revoke_all(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_audit<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth_policy::audit(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_audit_export<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth_policy::audit_export(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_provider_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::federation::provider_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_provider<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::federation::provider(&self.state, tenant).await
        })
    }

    pub(crate) fn arm_host_oauth_provider_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::federation::provider_remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_doctor<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::federation::doctor(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_trusted_issuer_set<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth::trusted_issuer_set(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_trusted_issuer_remove<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, args, .. } = c;
            crate::oauth::trusted_issuer_remove(&self.state, tenant, &args).await
        })
    }

    pub(crate) fn arm_host_oauth_trusted_issuers<'a>(&'a self, c: ArmCtx<'a>) -> ArmFut<'a> {
        box_arm(async move {
            let ArmCtx { tenant, .. } = c;
            crate::oauth::trusted_issuers_list(&self.state, tenant).await
        })
    }
}
