//! Boxed helpers for the verbs `call_tool_inner` answers in its own
//! `match (auth, name)` (PRD-mcphost-generated-dispatch-table-stack-budget R4):
//! `host.quickstart`, `host.help.search`, `host.help.get`, `host.errors`,
//! `host.feedback.*`, and the admin `host.call_trace` / `host.doctor`. Each body is
//! the arm it replaced, moved here so its future is built (and boxed) in this
//! function's frame instead of occupying a slot in `call_tool_inner`'s poll frame.

use super::*;

/// A helper's future, built on the heap.
type Boxed<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

impl McpHostHandler {
    /// `host.quickstart` for an authenticated tenant.
    pub(super) fn quickstart_tenant_boxed<'a>(
        &'a self,
        tenant: &'a Tenant,
        args: &'a Value,
        ctx: &'a RequestContext<RoleServer>,
        parts: &'a http::request::Parts,
    ) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(async move {
                // PRD-mcphost-client-install-links P0 requirement 3 (AC3):
                // `Some` only when THIS call authenticated over
                // `/u/{secret}/mcp` -- see `control::quickstart`'s own doc
                // comment on `path_url_secret` for why this is the only
                // auth path that can supply one.
                let path_url_secret = url_path_secret(parts);
                match control::quickstart(&self.state, Some(tenant), args, path_url_secret) {
                    Ok(mut result) => {
                        if let Some(recipe) = control::quickstart_alias_recipe(&result) {
                            control::record_recipe_use(&self.state, tenant.id, recipe, "quickstart").await;
                        }
                        // PRD-mcphost-quickstart-by-client R3/R6: the
                        // `client` block for the surface asking, rendered
                        // for the caller's own personal URL when this call
                        // came over `/u/{secret}/mcp`; the detected id is
                        // written to `tenants.client_name` when the
                        // tenant has none (best-effort, never overwrites).
                        let detected = quickstart_detection(ctx, parts);
                        let public = self.state.public_url.trim_end_matches('/');
                        let base = path_url_secret.map_or_else(|| public.to_string(), |secret| format!("{public}/u/{secret}"));
                        result["client"] = crate::install_links::client_block(&base, detected);
                        if tenant.client_name.is_none()
                            && let Some((id, _)) = detected
                        {
                            let _ = self.state.db.set_tenant_client_info(tenant.id, id.to_string(), String::new()).await;
                        }
                        // PRD-mcphost-composition requirement 6: once this
                        // tenant has at least two tools of its own,
                        // `host.quickstart(kind="chain")`'s example wires
                        // the placeholder two-step spec onto them by name
                        // instead of `ChainKind::example`'s static example
                        // (PRD-mcphost-chain-host-steps requirement 5: the
                        // python starter plus `host.table.append`) -- still
                        // just an example (`host.tool_publish` still has to
                        // be called to actually create it), but one that
                        // calls something the tenant already published.
                        // Best-effort: any lookup failure just leaves the
                        // generic placeholder spec.
                        if args.get("kind").and_then(Value::as_str) == Some("chain")
                            && let Ok(tools) = self.state.db.list_tools(tenant.id).await
                        {
                            let names: Vec<String> = tools
                                .into_iter()
                                .filter(|t| t.kind != "chain")
                                .map(|t| t.name)
                                .take(2)
                                .collect();
                            if let [first, second] = names.as_slice() {
                                result["steps"][0]["arguments"]["spec"] = json!({
                                    "steps": [
                                        {"tool": first, "args": {}},
                                        {"tool": second, "args": {}},
                                    ],
                                });
                            }
                        }
                        // PRD-mcphost-data-quota-in-every-response R2: the
                        // keyed tenant's full data-quota object, from the
                        // renderer every data verb's response uses.
                        if let Ok(Some(quota)) = control::data_quota_for(&self.state, tenant).await
                            && let Some(limits) = result.get_mut("limits").and_then(Value::as_object_mut)
                        {
                            limits.insert("quota".to_string(), quota);
                        }
                        // PRD-mcphost-docs-import-and-stated-boundary R3: a
                        // tenant with no documents yet reads what the docs
                        // store imports, and what it does not, before any
                        // instruction. Best-effort: a count failure leaves
                        // the field out.
                        if let Ok((0, _, _)) = self.state.db.documents_usage(tenant.id).await {
                            result["docs"] = json!({
                                "text": format!(
                                    "{} Load a corpus with host.docs.put_many (up to {} documents per call), then ask it with host.docs.search.",
                                    crate::docs::DOCS_BOUNDARY,
                                    crate::docs::PUT_MANY_MAX
                                ),
                                "call": "host.docs.put_many",
                            });
                        }
                        Ok(result)
                    }
                    Err(e) => Err(e),
                }
        })
    }

    /// `host.quickstart` for an anonymous caller.
    pub(super) fn quickstart_anonymous(
        &self,
        args: &Value,
        ctx: &RequestContext<RoleServer>,
        parts: &http::request::Parts,
    ) -> Result<Value, AppError> {
        control::quickstart(&self.state, None, args, None).map(|mut result| {
            // PRD-mcphost-quickstart-by-client R3: anonymous callers get
            // the same `client` block, rendered for the public endpoint.
            result["client"] =
                crate::install_links::client_block(&self.state.public_url, quickstart_detection(ctx, parts));
            result
        })
    }

    /// `host.help.search`; `tenant_id` is `None` for an anonymous caller.
    pub(super) fn help_search_boxed<'a>(
        &'a self,
        tenant_id: Option<i64>,
        args: &'a Value,
    ) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(async move {
                match crate::help_search::call_observed(&self.state.kinds, args) {
                    Ok((body, obs)) => {
                        if let Some(obs) = obs {
                            let tenant_id = tenant_id;
                            // Best-effort: telemetry never fails the search.
                            let _ = self
                                .state
                                .db
                                .record_help_search(tenant_id, obs.query_norm, obs.top_anchor, now_unix())
                                .await;
                        }
                        Ok(body)
                    }
                    Err(e) => Err(e),
                }
        })
    }

    /// `host.help.get`.
    pub(super) fn help_get_boxed<'a>(&'a self, args: &'a Value) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(async move { crate::doc_anchors::help_get(args) })
    }

    /// `host.errors`.
    pub(super) fn errors_boxed<'a>(&'a self, args: &'a Value) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(async move { crate::error_registry::call(&self.state.public_url, args) })
    }

    /// `host.feedback.send|list|get`. The outer `Err` is the early
    /// `NoDryRun` refusal, which answers the whole call (it skips the outcome
    /// post-processing), exactly as the inline `return` did.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn feedback_boxed<'a>(
        &'a self,
        tenant_id: Option<i64>,
        ctx: &'a RequestContext<RoleServer>,
        source: &'a str,
        feedback_ring_key: &'a str,
        body_name: &'a str,
        args: &'a Value,
    ) -> Boxed<'a, Result<Result<Value, AppError>, McpError>> {
        Box::pin(async move {
            Ok({
                let (client_name, client_version) = match peer_client_info(ctx) {
                    Some((n, v)) => (Some(n), Some(v)),
                    None => (None, None),
                };
                let sender = crate::feedback::Sender {
                    tenant_id,
                    source_ip: source,
                    client_name,
                    client_version,
                    ring_key: feedback_ring_key.to_string(),
                };
                if body_name == "host.feedback.send" {
                    // PRD-mcphost-dry-run-on-write-verbs: this pre-auth arm
                    // runs before the generic `dry_run` intercept, so the
                    // registry's NoDryRun refusal is applied here by hand.
                    if let Some(verb) = crate::write_verbs::lookup("host.feedback.send")
                        && crate::write_verbs::dry_run_requested(args)
                        && let crate::write_verbs::DryRun::NoDryRun(reason) = verb.dry_run
                    {
                        return Err(crate::write_verbs::no_dry_run_error(verb, reason).into_error_data());
                    }
                    crate::feedback::send(&self.state, &sender, args).await
                } else if body_name == "host.feedback.get" {
                    crate::feedback::get(&self.state, &sender, args).await
                } else {
                    crate::feedback::list(&self.state, &sender, args).await
                }
            })
        })
    }

    /// Admin `host.call_trace`: any tenant's trace.
    pub(super) fn call_trace_admin_boxed<'a>(&'a self, args: Value) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(async move {
            crate::precheck::precheck_args(&self.state.kinds, "host.call_trace", &args)?;
            self.call_trace(None, args).await
        })
    }

    /// Admin `host.doctor`.
    pub(super) fn doctor_admin_boxed<'a>(&'a self, args: Value) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(async move {
            crate::precheck::precheck_args(&self.state.kinds, "host.doctor", &args)?;
            self.doctor(None, args).await
        })
    }

    /// `dispatch_tenant_tool_tracking_prompts` under the calling-client scope
    /// and the caller's token scope (the host-verb arm); the bool is whether the prompt list changed.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_tenant_scoped_boxed<'a>(
        &'a self,
        call_scope: Option<String>,
        call_client: Option<String>,
        tenant: &'a Tenant,
        subject: Option<&'a str>,
        auth_method: &'a str,
        calls_auth_method: &'a str,
        end_user: Option<&'a crate::enduser::EndUser>,
        session_id: Option<&'a str>,
        name: &'a str,
        args: Value,
    ) -> Boxed<'a, (Result<Value, AppError>, bool)> {
        Box::pin(crate::scopes::CALL_SCOPE.scope(
            call_scope,
            crate::tenant_clients::CALL_CLIENT.scope(
                call_client,
                self.dispatch_tenant_tool_tracking_prompts(
                    tenant,
                    subject,
                    auth_method,
                    calls_auth_method,
                    end_user,
                    session_id,
                    name,
                    args,
                ),
            ),
        ))
    }

    /// A tool the caller published itself (`<own ns>.<tool>`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_own_published_boxed<'a>(
        &'a self,
        tenant: &'a Tenant,
        local_name: &'a str,
        args: Value,
        mcp_name_mismatch: bool,
        end_user: Option<&'a crate::enduser::EndUser>,
        auth_method: &'a str,
        token_scope: Option<&'a str>,
    ) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(self.call_published_tool(
            tenant,
            local_name,
            args,
            mcp_name_mismatch,
            None,
            None,
            end_user,
            auth_method,
            token_scope,
        ))
    }

    /// A tool another tenant shared with the caller (`<other ns>.<tool>`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_shared_boxed<'a>(
        &'a self,
        caller: &'a Tenant,
        owner_ns: &'a str,
        local_name: &'a str,
        args: Value,
        mcp_name_mismatch: bool,
        version: Option<i64>,
        end_user: Option<&'a crate::enduser::EndUser>,
        auth_method: &'a str,
    ) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(self.call_shared_tool(
            caller,
            owner_ns,
            local_name,
            args,
            mcp_name_mismatch,
            version,
            end_user,
            auth_method,
        ))
    }

    /// An `admin.*` verb.
    pub(super) fn dispatch_admin_boxed<'a>(&'a self, name: &'a str, args: Value) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(self.dispatch_admin_tool(name, args))
    }

    /// An anonymous `host.*` / `billing.*` call that signs the caller up first.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn implicit_signup_boxed<'a>(
        &'a self,
        ctx: &'a RequestContext<RoleServer>,
        parts: &'a http::request::Parts,
        source: &'a str,
        session_id: Option<&'a str>,
        name: &'a str,
        args: Value,
    ) -> Boxed<'a, Result<Value, AppError>> {
        Box::pin(self.implicit_signup_and_dispatch(ctx, parts, source, session_id, name, args))
    }
}
