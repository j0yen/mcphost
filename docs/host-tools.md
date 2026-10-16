# Write verbs and `dry_run`

Every host verb that mutates tenant data, from the `WRITE_VERBS` table in
`src/write_verbs.rs`. A `supported` verb called with `dry_run: true` runs inside a
rolled-back savepoint and answers `{would: {...}, _meta: {dry_run: true}}`; a `refused`
verb fails with `args_invalid` and `data.no_dry_run_reason` set to the reason beside it.
Regenerate with `scripts/sandbox-api-doc-check.sh --write`.

<!-- mcphost-write-verbs:start -->
| verb | kind | dry_run | no_dry_run_reason |
|---|---|---|---|
| `host.table.create` | write | supported | |
| `host.table.append` | write | supported | |
| `host.table.from_blob` | write | supported | |
| `host.table.drop` | drop | supported | |
| `host.table.handle_drop` | drop | supported | |
| `host.table.model_set` | write | supported | |
| `host.state.set` | write | supported | |
| `host.state.insert` | write | supported | |
| `host.state.delete` | delete | supported | |
| `host.state.delete_rows` | delete | supported | |
| `host.state.table_create` | write | supported | |
| `host.state.table_drop` | drop | supported | |
| `host.docs.put` | write | supported | |
| `host.docs.delete` | delete | supported | |
| `host.docs.purge` | delete | supported | |
| `host.index.create` | write | supported | |
| `host.index.delete` | delete | supported | |
| `host.enduser.purge` | delete | supported | |
| `host.enduser.revoke` | revoke | supported | |
| `host.enduser.unrevoke` | revoke | supported | |
| `host.oauth.revoke_all` | revoke | supported | |
| `host.oauth.grant_revoke` | revoke | supported | |
| `host.oauth.issuer_set` | write | supported | |
| `host.oauth.issuer_remove` | remove | supported | |
| `host.oauth.scope_set` | write | supported | |
| `host.oauth.policy_set` | write | supported | |
| `host.oauth.trusted_issuer_set` | write | supported | |
| `host.oauth.trusted_issuer_remove` | remove | supported | |
| `host.runs.purge` | delete | supported | |
| `host.tool.remove` | remove | supported | |
| `host.tool.publish` | publish | supported | |
| `host.trigger.set` | write | supported | |
| `host.trigger.pause` | write | supported | |
| `host.trigger.resume` | write | supported | |
| `host.trigger.remove` | remove | supported | |
| `host.group.create` | write | supported | |
| `host.group.remove` | remove | supported | |
| `host.invite.create` | write | supported | |
| `host.ask.create` | write | supported | |
| `host.invite.revoke` | revoke | supported | |
| `host.secret.set` | write | supported | |
| `host.policy.set` | write | supported | |
| `host.policy.attrs_set` | write | supported | |
| `host.agent.profile_set` | write | supported | |
| `host.share.caller_limit_remove` | remove | supported | |
| `host.key.rotate` | rotate | refused | `external_effect` |
| `host.key.mint` | write | refused | `external_effect` |
| `host.key.delegate` | write | refused | `external_effect` |
| `host.key.revoke` | revoke | refused | `irreversible_by_design` |
| `host.blob.put` | write | refused | `external_effect` |
| `host.blob.delete` | delete | refused | `external_effect` |
| `host.feedback.send` | write | refused | `external_effect` |
| `host.example.propose` | write | refused | `external_effect` |
| `host.self.offboard` | delete | refused | `irreversible_by_design` |
| `host.enduser.assertion_secret_rotate` | rotate | refused | `external_effect` |
| `host.registry.publish` | publish | refused | `external_effect` |
| `host.index.refresh` | write | refused | `external_effect` |
| `host.trigger.fire` | write | refused | `external_effect` |
| `host.msg.send` | write | refused | `external_effect` |
| `host.task.post` | write | refused | `external_effect` |
| `host.channel.post` | write | refused | `external_effect` |
| `host.uptime.create` | write | refused | `external_effect` |
| `host.webhook_inbox.create` | write | refused | `external_effect` |
| `host.vault.provider_set` | write | refused | `external_effect` |
| `host.vault.provider_remove` | remove | refused | `external_effect` |
| `host.oauth.provider_set` | write | refused | `external_effect` |
| `host.oauth.provider_remove` | remove | refused | `external_effect` |

### Confirm rule

- `host.table.drop`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.table.handle_drop`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.state.delete`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.state.delete_rows`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.state.table_drop`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.docs.delete`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.docs.purge`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.index.delete`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.enduser.purge`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.enduser.revoke`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.enduser.unrevoke`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.oauth.revoke_all`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.oauth.grant_revoke`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.oauth.issuer_remove`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.oauth.trusted_issuer_remove`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.runs.purge`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.tool.remove`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.trigger.remove`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.group.remove`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.invite.revoke`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
- `host.share.caller_limit_remove`: Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).
<!-- mcphost-write-verbs:end -->
