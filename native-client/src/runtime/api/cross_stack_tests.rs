use super::session::test_support::MemoryRefreshTokenStore;
use super::*;
use reqwest::{Method, Url};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::time::Duration;
use uuid::Uuid;

#[derive(serde::Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum JointControl<'a> {
    SetGroupStatus { group_id: &'a str, status: &'a str },
    SeedOwnerPages { group_id: &'a str, member_count: u32, invitation_count: u32, usage_event_count: u32 },
    HoldGenerationAdmission { task_id: &'a str, user_public_id: &'a str, account_group_id: &'a str },
    CompleteGenerationAdmission { task_id: &'a str, user_public_id: &'a str, account_group_id: &'a str, lease_handle: &'a str },
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JointFreeze { operation: String, group_id: String, status: String, group_version: String }
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JointSeed { operation: String, group_id: String, member_count: u32, invitation_count: u32, usage_event_count: u32 }
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JointHeld { operation: String, task_id: String, user_public_id: String, account_group_id: String, outcome: String, task_status: String, lease_handle: String, lease_epoch: String, saved_ceiling: u32, live_count: u32 }
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JointRejected { operation: String, task_id: String, user_public_id: String, account_group_id: String, outcome: String, task_status: String, reason: String, lease_epoch: String, saved_ceiling: u32, live_count: u32 }
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JointCompleted { operation: String, task_id: String, user_public_id: String, account_group_id: String, outcome: String, task_status: String, lease_handle: String, lease_epoch: String, released: bool, replayed: bool }
enum JointReceipt { Frozen(JointFreeze), Seeded(JointSeed), Held(JointHeld), Rejected(JointRejected), Completed(JointCompleted) }
fn joint_control(request: JointControl<'_>) -> JointReceipt {
    let base = base_url();
    // Validate before reading or attaching the control credential.
    assert_eq!(std::env::var("ARTFORGE_ENABLE_MOCK_API").as_deref(), Ok("1"));
    assert_eq!(base.as_str(), "http://127.0.0.1:39091/");
    assert!(base.username().is_empty() && base.password().is_none());
    let request = serde_json::to_value(request).unwrap();
    let token = std::env::var("ARTFORGE_MOCK_CONTROL_TOKEN").expect("isolated mock control credential");
    assert!(token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    let response = reqwest::blocking::Client::new().post(base.join("__mock__/team-account-fixtures").unwrap())
        .timeout(Duration::from_secs(10)).header("X-Mock-Control-Token", token).json(&request).send().unwrap();
    assert_eq!(response.status().as_u16(), 200, "isolated fixture operation rejected");
    let envelope: Value = response.json().unwrap();
    let data = envelope.get("data").cloned().expect("fixture envelope data");
    assert_eq!(data["operation"], request["operation"]);
    match request["operation"].as_str().unwrap() {
        "set_group_status" => JointReceipt::Frozen(serde_json::from_value(data).unwrap()),
        "seed_owner_pages" => JointReceipt::Seeded(serde_json::from_value(data).unwrap()),
        "hold_generation_admission" if data["outcome"] == "held" => JointReceipt::Held(serde_json::from_value(data).unwrap()),
        "hold_generation_admission" => JointReceipt::Rejected(serde_json::from_value(data).unwrap()),
        "complete_generation_admission" => JointReceipt::Completed(serde_json::from_value(data).unwrap()),
        _ => unreachable!(),
    }
}
fn joint_scope(client: &ApiClient, login: &LoginResponse) -> SessionScope {
    client.session().scope_for_user(&login.user.id).unwrap()
}
fn joint_owned(client: &ApiClient, scope: &SessionScope) -> AccountGroupChoice {
    TeamApi::new(client.clone()).list_groups(scope).unwrap().items.into_iter().find(|choice| choice.role == "owner").unwrap()
}
fn joint_billing(scope: &SessionScope, choice: &AccountGroupChoice) -> BillingScope {
    BillingScope { request: GroupRequestScope { session: scope.clone(), account_group_id: choice.group_id.clone() }, context_epoch: 1 }
}
fn joint_confirm(client: &ApiClient, manager: &BillingContextManager, writer: &crate::runtime::ClientStateWriter, choice: AccountGroupChoice, scope: &SessionScope, rollback: PreviousBillingAuthority) -> BillingScope {
    let ticket = manager.begin_switch(scope, &client.device().id, &choice.group_id, rollback).unwrap();
    assert!(manager.confirmed_scope().is_none());
    let snapshot = AccountApi::new(client.clone()).snapshot_billing(ticket.proposed_scope()).unwrap();
    let staged = manager.stage_confirmation(&ticket, choice, snapshot.account).unwrap();
    writer.save_selected_group(&scope.owner_user_id, &client.device().id, &ticket.proposed_scope().request.account_group_id).unwrap();
    let confirmed = ticket.proposed_scope().clone();
    manager.publish_persisted(ticket, staged);
    assert_eq!(manager.confirmed_scope(), Some(confirmed.clone()));
    assert_eq!(writer.load_selected_group(&scope.owner_user_id, &client.device().id).unwrap().as_deref(), Some(confirmed.request.account_group_id.as_str()));
    confirmed
}
struct JointMembership {
    owner: ApiClient, owner_scope: SessionScope, owner_group: AccountGroupChoice,
    member: ApiClient, member_scope: SessionScope, personal_group: AccountGroupChoice, membership: MemberView,
}
fn joint_membership(prefix: &str) -> JointMembership {
    let (owner, owner_login, _) = login_new_user_with_email(&format!("{prefix}-owner"));
    let (member, member_login, email) = login_new_user_with_email(&format!("{prefix}-member"));
    let owner_scope = joint_scope(&owner, &owner_login); let member_scope = joint_scope(&member, &member_login);
    let owner_group = joint_owned(&owner, &owner_scope); let personal_group = joint_owned(&member, &member_scope);
    let invitation = TeamApi::new(owner.clone()).create_invitation(&owner_group.group_id, &email, "500", &Uuid::new_v4().to_string(), &owner_scope).unwrap();
    let membership = TeamApi::new(member.clone()).accept_invitation(&invitation.invitation_id, &invitation.version, &Uuid::new_v4().to_string(), &member_scope).unwrap();
    JointMembership { owner, owner_scope, owner_group, member, member_scope, personal_group, membership }
}
fn joint_selected_member(fixture: &JointMembership) -> AccountGroupChoice {
    TeamApi::new(fixture.member.clone()).list_groups(&fixture.member_scope).unwrap().items.into_iter()
        .find(|choice| choice.group_id == fixture.owner_group.group_id).unwrap()
}
fn joint_exact_keys(value: &Value, keys: &[&str]) {
    let actual = value.as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>();
    let expected = keys.iter().copied().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_invited_registration_outcomes() {
    for count in [1, 2] {
        let client = new_client(); let auth = AuthApi::new(client.clone());
        let email = format!("joint-invited-{count}-{}@example.com", Uuid::new_v4());
        let mut owners = Vec::new();
        for index in 0..count {
            let (owner, login, _) = login_new_user_with_email(&format!("joint-inviter-{index}"));
            let scope = joint_scope(&owner, &login); let choice = joint_owned(&owner, &scope);
            let invitation = TeamApi::new(owner.clone()).create_invitation(&choice.group_id, &email, "123", &Uuid::new_v4().to_string(), &scope).unwrap();
            owners.push((owner, scope, choice, invitation));
        }
        let acceptances = agreement_acceptances(&auth);
        auth.request_email_code(&email).unwrap();
        let outcome = auth.login_response(&email, &mock_code(), &acceptances).unwrap();
        let EmailLoginOutcome::TeamRegistrationRequired { registration_continuation, pending_invitation_count, invitations, selection_state, .. } = outcome else { panic!("invited user must not receive session or grant"); };
        assert_eq!(pending_invitation_count, count as u64); assert_eq!(invitations.len(), count);
        assert!(client.session().access().is_none());
        let key = Uuid::new_v4().to_string();
        let body = serde_json::to_value(TeamRegistrationRequest {
            registration_continuation: registration_continuation.expose(), password: PASSWORD_ALPHA,
            agreement_acceptances: &acceptances, device_id: &client.device().id, device_name: &client.device().name,
            platform: &client.device().platform, app_version: client.app_version(),
        }).unwrap();
        let first = client.public_json_idempotent::<Value>(Method::POST, "/v1/auth/team-registration", Some(body.clone()), &key).unwrap().data;
        joint_exact_keys(&first, &["user","group_choices","selection_state","suggested_account_group_id","access_token","access_expires_in_seconds","refresh_token","refresh_expires_at","token_type"]);
        let parsed: TeamRegistrationResult = serde_json::from_value(first.clone()).unwrap();
        let TeamRegistrationSessionResult::Authenticated { tokens } = parsed.session else { panic!("first registration needs actual tokens"); };
        let replay = client.public_json_idempotent::<Value>(Method::POST, "/v1/auth/team-registration", Some(body), &key).unwrap().data;
        joint_exact_keys(&replay, &["user","group_choices","selection_state","suggested_account_group_id","session_login_required"]);
        assert_eq!(replay["session_login_required"], true);
        for field in ["user","group_choices","selection_state","suggested_account_group_id"] { assert_eq!(first[field], replay[field]); }
        let typed_replay = auth.complete_team_registration(&registration_continuation, PASSWORD_ALPHA, &acceptances, &key).unwrap();
        assert!(matches!(typed_replay.session, TeamRegistrationSessionResult::LoginRequired { session_login_required: true }));
        // Same normal password request used by the replay callback, never synthetic tokens.
        let login = auth.password_login_response(&email, PASSWORD_ALPHA, &acceptances).unwrap();
        assert_eq!(login.user.id, parsed.user.id); assert!(!login.is_new_user);
        let scope = client.session().install_tokens_for_user(&login.tokens, &login.user.id).unwrap();
        let owned = joint_owned(&client, &scope);
        let personal = AccountApi::new(client.clone()).snapshot_billing(&joint_billing(&scope, &owned)).unwrap();
        assert_eq!(personal.account.credits.unwrap().lifetime_granted, "0");
        assert!(!personal.account.user.invitation_code_submitted);
        if count == 1 {
            assert_eq!(selection_state, TeamRegistrationSelectionState::Unique);
            assert_eq!(parsed.suggested_account_group_id.as_deref(), Some(owners[0].2.group_id.as_str()));
        } else {
            assert_eq!(selection_state, TeamRegistrationSelectionState::Multiple);
            assert!(parsed.suggested_account_group_id.is_none());
            let pending = TeamApi::new(client.clone()).list_pending_invitations(None, &scope).unwrap();
            assert_eq!(pending.items.len(), 2);
            let selected = &pending.items[0];
            TeamApi::new(client.clone()).accept_invitation(&selected.invitation_id, &selected.version, &Uuid::new_v4().to_string(), &scope).unwrap();
            assert!(TeamApi::new(client.clone()).list_pending_invitations(None, &scope).unwrap().items.is_empty());
            let alternate = owners.iter().find(|(_,_,choice,_)| choice.group_id != selected.group_id).unwrap();
            let rows = TeamApi::new(alternate.0.clone()).list_invitations(&alternate.2.group_id, None, &alternate.1).unwrap();
            assert_eq!(rows.items.iter().find(|row| row.invitation_id == alternate.3.invitation_id).unwrap().status, "superseded");
        }
        drop(tokens);
        auth.logout_scoped(false, &scope).unwrap();
        for (owner, scope, _, _) in owners { AuthApi::new(owner).logout_scoped(false, &scope).unwrap(); }
    }
}
#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_team_selection_and_frozen_fallback() {
    let fixture = joint_membership("joint-selection");
    let writer = crate::runtime::client_state::tests::Fixture::new(false, false);
    let manager = BillingContextManager::with_upgrade_latch(fixture.member.upgrade_latch().clone());
    manager.bind_authenticated_session(fixture.member_scope.clone()).unwrap();
    let personal = joint_confirm(&fixture.member, &manager, &writer, fixture.personal_group.clone(), &fixture.member_scope, PreviousBillingAuthority::StillValid);
    let joined = joint_confirm(&fixture.member, &manager, &writer, joint_selected_member(&fixture), &fixture.member_scope, PreviousBillingAuthority::StillValid);
    assert_ne!(personal.request.account_group_id, joined.request.account_group_id);
    assert!(manager.current_scope(KnownCapability::ManageGroup).is_err());
    TeamApi::new(fixture.owner.clone()).update_member(&fixture.owner_group.group_id, &fixture.membership.member_id, MemberPolicyAction::Suspend, &fixture.membership.version, &Uuid::new_v4().to_string(), &fixture.owner_scope).unwrap();
    let groups = TeamApi::new(fixture.member.clone()).list_groups(&fixture.member_scope).unwrap();
    assert!(!groups.items.iter().find(|choice| choice.group_id == fixture.owner_group.group_id).unwrap().selectable);
    manager.invalidate_current_billing(); assert!(manager.billable_scope().is_err());
    let fallback = BillingContextManager::choose_candidate(&groups.items, None, None).unwrap().clone();
    joint_confirm(&fixture.member, &manager, &writer, fallback, &fixture.member_scope, PreviousBillingAuthority::Invalidated);
    assert_eq!(manager.confirmed_scope().unwrap().request.account_group_id, fixture.personal_group.group_id);
    let JointReceipt::Frozen(receipt) = joint_control(JointControl::SetGroupStatus { group_id: &fixture.personal_group.group_id, status: "frozen" }) else { panic!("freeze receipt"); };
    assert_eq!(receipt.status, "frozen"); assert_eq!(receipt.group_id, fixture.personal_group.group_id); assert!(receipt.group_version.parse::<i64>().is_ok());
    let groups = TeamApi::new(fixture.member.clone()).list_groups(&fixture.member_scope).unwrap();
    let fallback = BillingContextManager::choose_candidate(&groups.items, None, None).unwrap().clone();
    assert!(fallback.readable_context && !fallback.selectable);
    let frozen = joint_confirm(&fixture.member, &manager, &writer, fallback, &fixture.member_scope, PreviousBillingAuthority::Invalidated);
    assert!(manager.confirmed_snapshot().unwrap().read_only);
    let before = fixture.member.test_request_receipts().len();
    assert!(manager.billable_scope().is_err()); assert!(manager.current_scope(KnownCapability::Purchase).is_err());
    assert_eq!(fixture.member.test_request_receipts().len(), before);
    assert_http_error(GenerationApi::new(fixture.member.clone()).create_task_billing(&prompt_request(Uuid::new_v4().to_string(), "frozen"), &frozen), 409, "account_group_frozen");
    AuthApi::new(fixture.member).logout_scoped(false, &fixture.member_scope).unwrap();
    AuthApi::new(fixture.owner).logout_scoped(false, &fixture.owner_scope).unwrap();
}
#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_team_privacy_and_pagination() {
    let (owner, login, _) = login_new_user_with_email("joint-pages"); let scope = joint_scope(&owner, &login); let group = joint_owned(&owner, &scope);
    let JointReceipt::Seeded(receipt) = joint_control(JointControl::SeedOwnerPages { group_id: &group.group_id, member_count: 51, invitation_count: 51, usage_event_count: 51 }) else { panic!("seed receipt"); };
    assert_eq!((receipt.member_count, receipt.invitation_count, receipt.usage_event_count), (51,51,51));
    let api = TeamApi::new(owner.clone());
    let members = api.list_members(&group.group_id, None, &scope).unwrap(); assert_eq!(members.items.len(), 50);
    let members2 = api.list_members(&group.group_id, members.next_cursor.as_deref(), &scope).unwrap(); assert_eq!(members2.items.len(), 1); assert!(members2.next_cursor.is_none());
    let ids = members.items.iter().chain(&members2.items).map(|row| row.member_id.as_str()).collect::<HashSet<_>>(); assert_eq!(ids.len(), 51);
    let invitations = api.list_invitations(&group.group_id, None, &scope).unwrap(); assert_eq!(invitations.items.len(), 50);
    let invitations2 = api.list_invitations(&group.group_id, invitations.next_cursor.as_deref(), &scope).unwrap(); assert_eq!(invitations2.items.len(), 1); assert!(invitations2.next_cursor.is_none());
    assert_eq!(invitations.items.iter().chain(&invitations2.items).map(|row| row.invitation_id.as_str()).collect::<HashSet<_>>().len(), 51);
    let usage = api.usage_page(&group.group_id, None, &scope).unwrap(); assert_eq!(usage.items.len(), 50);
    let usage2 = api.usage_page(&group.group_id, usage.next_cursor.as_deref(), &scope).unwrap(); assert_eq!(usage2.items.len(), 1); assert!(usage2.next_cursor.is_none());
    assert_eq!(usage.items.iter().chain(&usage2.items).map(|row| row.usage_event_id.as_str()).collect::<HashSet<_>>().len(), 51);
    let raw = owner.identity_json_scoped::<Value>(Method::GET, &format!("/v1/account-groups/{}/usage?page_size=50", group.group_id), None, None, &scope).unwrap().data;
    for row in raw["items"].as_array().unwrap() {
        joint_exact_keys(row, &["usage_event_id","member","occurred_at","period_start","period_end","operation_kind","model_label","credit_amount","phase","outcome"]);
        joint_exact_keys(&row["member"], &["user_id","display_name","email_masked"]);
    }
    let fixture = joint_membership("joint-privacy");
    let selected = joint_billing(&fixture.member_scope, &joint_selected_member(&fixture));
    let raw = fixture.member.billing_json_scoped::<Value>(Method::GET, "/v1/account", None, None, &selected).unwrap().data;
    for forbidden in ["credits","membership","orders","wallet","files","prompts"] { assert!(raw.get(forbidden).is_none()); }
    assert!(raw["quota"].is_object()); assert_eq!(raw["user"]["id"], fixture.member_scope.owner_user_id);
    assert!(raw["entitlement"].is_object()); assert_eq!(raw["read_only"], false);
    for path in [format!("/v1/account-groups/{}/usage?page_size=50", fixture.owner_group.group_id), format!("/v1/account-groups/{}/billing-summary", fixture.owner_group.group_id)] {
        assert!(matches!(fixture.member.identity_json_scoped::<Value>(Method::GET, &path, None, None, &fixture.member_scope), Err(ApiError::Http { status: 403, .. })));
    }
    AuthApi::new(owner).logout_scoped(false, &scope).unwrap();
    AuthApi::new(fixture.owner).logout_scoped(false, &fixture.owner_scope).unwrap();
    AuthApi::new(fixture.member).logout_scoped(false, &fixture.member_scope).unwrap();
}

#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_team_header_matrix() {
    let fixture = joint_membership("joint-headers");
    let scope = joint_billing(&fixture.owner_scope, &fixture.owner_group);
    for path in ["/v1/account", "/v1/credits/account", "/v1/credits/ledger?limit=8", "/v1/credits/packs", "/v1/membership/plans", "/v1/models", "/v1/orders?page_size=50"] {
        assert_http_error(fixture.owner.identity_json_scoped::<Value>(Method::GET, path, None, None, &fixture.owner_scope), 400, "account_group_context_required");
        fixture.owner.billing_json_scoped::<Value>(Method::GET, path, None, None, &scope).unwrap();
    }
    let request = prompt_request(Uuid::new_v4().to_string(), "joint header content");
    assert_http_error(GenerationApi::new(fixture.owner.clone()).create_task_scoped(&request, &fixture.owner_scope), 400, "account_group_context_required");
    let task = GenerationApi::new(fixture.owner.clone()).create_task_billing(&request, &scope).unwrap();
    require_saved_group(&scope.request.account_group_id, &task.billing_account_group_id).unwrap();
    GenerationApi::new(fixture.owner.clone()).task_scoped(&task.id, &fixture.owner_scope).unwrap();
    GenerationApi::new(fixture.owner.clone()).cancel_scoped(&task.id, &fixture.owner_scope).unwrap();
    TeamApi::new(fixture.owner.clone()).list_groups(&fixture.owner_scope).unwrap();
    TeamApi::new(fixture.owner.clone()).list_members(&fixture.owner_group.group_id, None, &fixture.owner_scope).unwrap();
    TeamApi::new(fixture.owner.clone()).list_invitations(&fixture.owner_group.group_id, None, &fixture.owner_scope).unwrap();
    TeamApi::new(fixture.owner.clone()).usage_page(&fixture.owner_group.group_id, None, &fixture.owner_scope).unwrap();
    TeamApi::new(fixture.owner.clone()).billing_summary(&fixture.owner_group.group_id, &fixture.owner_scope).unwrap();
    TeamApi::new(fixture.member.clone()).own_membership(&fixture.owner_group.group_id, &fixture.member_scope).unwrap();
    TeamApi::new(fixture.member.clone()).list_pending_invitations(None, &fixture.member_scope).unwrap();
    let payment = PaymentApi::new(fixture.owner.clone());
    let packs = payment.packs_billing(&scope).unwrap(); let pack = &packs[0];
    let key = Uuid::new_v4().to_string();
    assert_http_error(payment.create_credit_order_scoped(&pack.code, &key, &fixture.owner_scope), 400, "account_group_context_required");
    let order = payment.create_credit_order_billing(&pack.code, &key, &scope).unwrap();
    payment.order_scoped(&order.id, &fixture.owner_scope).unwrap(); payment.sync_order_scoped(&order.id, &fixture.owner_scope).unwrap();
    let receipts = fixture.owner.test_request_receipts();
    for receipt in receipts.iter().filter(|receipt| receipt.path.starts_with("/v1/account-groups") || receipt.path.starts_with("/v1/account/sessions") || receipt.path == format!("/v1/generation/tasks/{}", task.id) || receipt.path == format!("/v1/generation/tasks/{}/cancel", task.id) || receipt.path.starts_with(&format!("/v1/orders/{}", order.id))) {
        assert!(receipt.selected_group.is_none()); assert!(receipt.has_auth);
    }
    assert!(receipts.iter().any(|receipt| receipt.path == "/v1/generation/tasks" && receipt.method == "POST" && receipt.selected_group.as_deref() == Some(scope.request.account_group_id.as_str())));
    assert!(receipts.iter().filter(|receipt| receipt.path.starts_with("/v1/auth/email/")).all(|receipt| receipt.selected_group.is_none() && !receipt.has_auth));
    AuthApi::new(fixture.owner).logout_scoped(false, &fixture.owner_scope).unwrap();
    AuthApi::new(fixture.member).logout_scoped(false, &fixture.member_scope).unwrap();
}
struct JointClaims(Vec<JointHeld>);
impl JointClaims {
    fn complete(receipt: &JointHeld) -> JointCompleted {
        let JointReceipt::Completed(completed) = joint_control(JointControl::CompleteGenerationAdmission {
            task_id: &receipt.task_id, user_public_id: &receipt.user_public_id, account_group_id: &receipt.account_group_id, lease_handle: &receipt.lease_handle,
        }) else { panic!("completion receipt"); };
        assert_eq!(completed.operation, "complete_generation_admission");
        assert_eq!(completed.task_id, receipt.task_id);
        assert_eq!(completed.user_public_id, receipt.user_public_id);
        assert_eq!(completed.account_group_id, receipt.account_group_id);
        assert_eq!(completed.outcome, "completed");
        assert_eq!(completed.task_status, "completed");
        assert_eq!(completed.lease_handle, receipt.lease_handle);
        assert_eq!(completed.lease_epoch, receipt.lease_epoch);
        assert!(completed.released);
        completed
    }
    fn finish(mut self) {
        while let Some(receipt) = self.0.last() {
            let completed = Self::complete(receipt);
            assert!(!completed.replayed, "normal drain must release each remaining handle once");
            self.0.pop();
        }
    }
}
impl Drop for JointClaims {
    fn drop(&mut self) {
        for receipt in self.0.drain(..) {
            let _ = std::panic::catch_unwind(|| { let _ = Self::complete(&receipt); });
        }
    }
}
#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_user_concurrency_across_groups() {
    let fixture = joint_membership("joint-concurrency");
    let personal = joint_billing(&fixture.member_scope, &fixture.personal_group);
    let joined = joint_billing(&fixture.member_scope, &joint_selected_member(&fixture));
    let api = GenerationApi::new(fixture.member.clone());
    let create = |billing: &BillingScope| {
        let task = api.create_task_billing(&prompt_request(Uuid::new_v4().to_string(), "queued joint task"), billing).unwrap();
        assert_eq!(task.status, "queued"); task
    };
    let first = create(&personal);
    let JointReceipt::Held(first_claim) = joint_control(JointControl::HoldGenerationAdmission {
        task_id: &first.id, user_public_id: &fixture.member_scope.owner_user_id, account_group_id: &personal.request.account_group_id,
    }) else { panic!("first user admission"); };
    assert!(first_claim.saved_ceiling > 0 && first_claim.saved_ceiling <= 64);
    let ceiling = first_claim.saved_ceiling; let epoch = first_claim.lease_epoch.clone();
    let mut claims = JointClaims(vec![first_claim]);
    for index in 1..ceiling {
        let scope = if index % 2 == 0 { &personal } else { &joined }; let task = create(scope);
        let JointReceipt::Held(receipt) = joint_control(JointControl::HoldGenerationAdmission { task_id: &task.id, user_public_id: &fixture.member_scope.owner_user_id, account_group_id: &scope.request.account_group_id }) else { panic!("admission below saved ceiling"); };
        assert_eq!(receipt.lease_epoch, epoch); assert_eq!(receipt.saved_ceiling, ceiling); assert_eq!(receipt.live_count, index + 1);
        claims.0.push(receipt);
    }
    let extra = create(&joined);
    let JointReceipt::Rejected(rejected) = joint_control(JointControl::HoldGenerationAdmission { task_id: &extra.id, user_public_id: &fixture.member_scope.owner_user_id, account_group_id: &joined.request.account_group_id }) else { panic!("one user must share a ceiling across groups"); };
    assert_eq!(rejected.reason, "user_concurrency_exceeded"); assert_eq!(rejected.task_status, "queued"); assert_eq!(rejected.live_count, ceiling); assert_eq!(rejected.lease_epoch, epoch);
    let owner_billing = joint_billing(&fixture.owner_scope, &fixture.owner_group);
    let other = GenerationApi::new(fixture.owner.clone()).create_task_billing(&prompt_request(Uuid::new_v4().to_string(), "other real user"), &owner_billing).unwrap();
    let JointReceipt::Held(other_claim) = joint_control(JointControl::HoldGenerationAdmission { task_id: &other.id, user_public_id: &fixture.owner_scope.owner_user_id, account_group_id: &owner_billing.request.account_group_id }) else { panic!("second user independently admits"); };
    claims.0.push(other_claim);
    let writer = crate::runtime::client_state::tests::Fixture::new(false, false);
    let manager = BillingContextManager::with_upgrade_latch(fixture.member.upgrade_latch().clone());
    manager.bind_authenticated_session(fixture.member_scope.clone()).unwrap();
    joint_confirm(&fixture.member, &manager, &writer, joint_selected_member(&fixture), &fixture.member_scope, PreviousBillingAuthority::StillValid);
    let completed = JointClaims::complete(&claims.0[0]); assert!(!completed.replayed);
    let replay = JointClaims::complete(&claims.0[0]); assert!(replay.replayed);
    assert_eq!(completed.lease_epoch, replay.lease_epoch);
    claims.0.remove(0);
    let JointReceipt::Held(extra_claim) = joint_control(JointControl::HoldGenerationAdmission { task_id: &extra.id, user_public_id: &fixture.member_scope.owner_user_id, account_group_id: &joined.request.account_group_id }) else { panic!("released slot should admit once"); };
    assert_eq!(extra_claim.lease_epoch, epoch); assert_eq!(extra_claim.live_count, ceiling);
    claims.0.push(extra_claim);
    claims.finish();
    AuthApi::new(fixture.owner).logout_scoped(false, &fixture.owner_scope).unwrap();
    AuthApi::new(fixture.member).logout_scoped(false, &fixture.member_scope).unwrap();
}
#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_immutable_billing_recovery() {
    use crate::runtime::*;
    let fixture = joint_membership("joint-recovery");
    let writer = client_state::tests::Fixture::new(false, false);
    let manager = BillingContextManager::with_upgrade_latch(fixture.member.upgrade_latch().clone());
    manager.bind_authenticated_session(fixture.member_scope.clone()).unwrap();
    let a = joint_confirm(&fixture.member, &manager, &writer, fixture.personal_group.clone(), &fixture.member_scope, PreviousBillingAuthority::StillValid);
    let lease = writer.lease(&fixture.member_scope.owner_user_id, fixture.member_scope.auth_epoch, 1);
    let authority = NamespaceStorageAuthority::open(writer.data_root_capability_arc(), &lease).unwrap();
    let request = prompt_request(Uuid::new_v4().to_string(), "immutable creation content");
    let mut record: PendingGenerationRecord = serde_json::from_value(json!({
        "schema_version":2,"client_request_id":request.client_request_id,"owner_user_id":fixture.member_scope.owner_user_id,
        "billing_account_group_id":a.request.account_group_id,"auth_epoch":fixture.member_scope.auth_epoch,
        "local_task_id":Uuid::new_v4().to_string(),"raw_prompt":request.prompt,"generation_prompt":request.prompt,
        "task_type":request.task_type,"category":"game","mode":"game","ratio":"square","quality":"1K",
        "model_code":request.model_code,"conversation_id":"","count":1,"create_conversation":false
    })).unwrap();
    upsert_pending_generation_for_namespace(&authority, &a, record.clone()).unwrap();
    let generation = GenerationApi::new(fixture.member.clone()).create_task_billing(&request, &a).unwrap();
    require_saved_group(&record.billing_account_group_id, &generation.billing_account_group_id).unwrap();
    apply_generation_patch_for_namespace(&authority, &record.identity(), GenerationRecoveryPatch::Accepted { server_task_id: generation.id.clone(), uploaded_file_ids: Vec::new(), clear_reference_inputs: true }).unwrap();
    record.server_task_id = generation.id.clone();
    let deep_request = CreatePromptOptimization { client_request_id: Uuid::new_v4().to_string(), prompt: "a woodland village".into(), run_mode:"auto".into(),focus_mode:"system".into(),max_rounds:2,target_score:90 };
    let deep_record = PendingPromptOptimizationRecord { schema_version:2,client_request_id:deep_request.client_request_id.clone(),owner_user_id:fixture.member_scope.owner_user_id.clone(),auth_epoch:fixture.member_scope.auth_epoch,billing_account_group_id:a.request.account_group_id.clone(),server_job_id:String::new(),presentation_dismissed:false,operation:PendingPromptOptimizationOperation::Create { request:deep_request.clone() } };
    upsert_pending_prompt_optimization_for_namespace(&authority, &a, deep_record.clone()).unwrap();
    let deep = PromptOptimizationApi::new(fixture.member.clone()).create_billing(&deep_request, &a).unwrap();
    require_saved_group(&a.request.account_group_id, &deep.billing_account_group_id).unwrap();
    let payment = PaymentApi::new(fixture.member.clone()); let pack = payment.packs_billing(&a).unwrap().remove(0); let key = Uuid::new_v4().to_string();
    let order_record = PendingOrderRecord { schema_version:2,kind:"credit".into(),client_request_id:key.clone(),owner_user_id:fixture.member_scope.owner_user_id.clone(),billing_account_group_id:a.request.account_group_id.clone(),auth_epoch:fixture.member_scope.auth_epoch,order_id:String::new(),product_code:pack.code.clone(),upgrade_quote_id:String::new(),created_at:chrono::Utc::now().to_rfc3339() };
    upsert_pending_order_for_namespace(&authority, &a, order_record).unwrap();
    let order = payment.create_credit_order_billing(&pack.code, &key, &a).unwrap();
    let b = joint_confirm(&fixture.member, &manager, &writer, joint_selected_member(&fixture), &fixture.member_scope, PreviousBillingAuthority::StillValid);
    assert_ne!(a.request.account_group_id, b.request.account_group_id);
    let detail = GenerationApi::new(fixture.member.clone()).task_scoped(&generation.id, &fixture.member_scope).unwrap();
    require_saved_group(&record.billing_account_group_id, &detail.billing_account_group_id).unwrap();
    let deep_detail = PromptOptimizationApi::new(fixture.member.clone()).get_scoped(&deep.id, &fixture.member_scope).unwrap();
    require_saved_group(&a.request.account_group_id, &deep_detail.billing_account_group_id).unwrap();
    for saved in [payment.order_scoped(&order.id, &fixture.member_scope).unwrap(), payment.sync_order_scoped(&order.id, &fixture.member_scope).unwrap()] {
        require_saved_group(&a.request.account_group_id, &saved.billing_account_group_id).unwrap();
    }
    let before = load_pending_generations_for_namespace(&authority).unwrap();
    let mut corrupt = before[0].clone(); corrupt.billing_account_group_id = b.request.account_group_id.clone();
    assert!(require_saved_group(&corrupt.billing_account_group_id, &detail.billing_account_group_id).is_err());
    assert!(upsert_pending_generation_for_namespace(&authority, &b, corrupt).is_err());
    let after = load_pending_generations_for_namespace(&authority).unwrap();
    assert_eq!(serde_json::to_value(&before).unwrap(), serde_json::to_value(&after).unwrap());
    assert_eq!(load_pending_orders_for_namespace(&authority).unwrap()[0].billing_account_group_id, a.request.account_group_id);
    assert_eq!(load_pending_prompt_optimizations_for_namespace(&authority).unwrap()[0].billing_account_group_id, a.request.account_group_id);
    GenerationApi::new(fixture.member.clone()).cancel_scoped(&generation.id, &fixture.member_scope).unwrap();
    PromptOptimizationApi::new(fixture.member.clone()).cancel_scoped(&deep.id, &fixture.member_scope).unwrap();
    drop(authority);
    AuthApi::new(fixture.member).logout_scoped(false, &fixture.member_scope).unwrap();
    AuthApi::new(fixture.owner).logout_scoped(false, &fixture.owner_scope).unwrap();
}
#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_account_reauthentication() {
    let (client, login, email) = login_new_user_with_email("joint-reauth");
    let scope = joint_scope(&client, &login); let before = client.session().access().unwrap();
    let auth = AuthApi::new(client.clone());
    let login_delivery = auth.request_email_code(&email).unwrap();
    assert!((1..=60).contains(&login_delivery.resend_after_seconds), "bounded server resend interval");
    assert!(login_delivery.expires_in_seconds > login_delivery.resend_after_seconds + 5,
        "the unconsumed login-purpose code must remain valid beyond cooldown");
    let login_code_received = std::time::Instant::now();
    let cooldown = Duration::from_secs(login_delivery.resend_after_seconds + 1);
    while login_code_received.elapsed() < cooldown {
        std::thread::sleep(cooldown.saturating_sub(login_code_received.elapsed()).min(Duration::from_millis(250)));
    }
    let api = TeamApi::new(client.clone());
    let delivery = api.request_reauthentication_code(&scope).unwrap();
    assert!(delivery.expires_in_seconds > 0 && delivery.resend_after_seconds > 0);
    let code = std::env::var("ARTFORGE_MOCK_REAUTH_CODE").unwrap_or_else(|_| MOCK_PASSWORD_CODE.to_string());
    assert_ne!(mock_code(), code, "isolated login and reauthentication fixtures use distinct codes");
    assert!(login_code_received.elapsed() < Duration::from_secs(login_delivery.expires_in_seconds - 1),
        "the login code is still valid and has never been consumed");
    // A real reauthentication challenge now exists; a valid login-purpose code
    // must not satisfy it, and the rejection must not consume the correct proof.
    assert_http_error(api.reauthenticate(ReauthenticationRequest::email_code(&mock_code()), &scope),
        400, "email_code_invalid");
    let proof = api.reauthenticate(ReauthenticationRequest::email_code(&code), &scope).unwrap();
    assert_eq!(proof.user_id, scope.owner_user_id);
    assert!(chrono::DateTime::parse_from_rfc3339(&proof.reauthenticated_at).unwrap() < chrono::DateTime::parse_from_rfc3339(&proof.expires_at).unwrap());
    let after = client.session().access().unwrap();
    assert_eq!(before.access_token, after.access_token); assert_eq!(before.auth_epoch, after.auth_epoch);
    assert!(client.session().is_scope_current(&scope));
    let receipts = client.test_request_receipts();
    assert!(receipts.iter().filter(|receipt| receipt.path.starts_with("/v1/account/reauth")).all(|receipt| receipt.selected_group.is_none() && receipt.has_auth));
    auth.logout_scoped(false, &scope).unwrap();
}
#[test]
#[ignore = "requires the isolated team Mock API on 127.0.0.1:39091"]
fn cross_stack_global_upgrade_required() {
    let (current, login, _) = login_new_user_with_email("joint-upgrade");
    let current_scope = joint_scope(&current, &login); let owned = joint_owned(&current, &current_scope);
    for identity in [true, false] {
        let old = new_client_with(current.device().id.clone(), "0.0.0");
        let scope = old.session().install_tokens_for_user(&login.tokens, &login.user.id).unwrap();
        let sibling = old.clone();
        let error = if identity {
            old.identity_json_scoped::<Value>(Method::GET, "/v1/account-groups", None, None, &scope).unwrap_err()
        } else {
            old.billing_json_scoped::<Value>(Method::GET, "/v1/account", None, None, &joint_billing(&scope, &owned)).unwrap_err()
        };
        assert!(matches!(error, ApiError::Http { status:426, ref code, .. } if code == "client_upgrade_required"));
        assert!(old.upgrade_latch().is_tripped());
        let before = old.test_request_receipts().len();
        assert!(sibling.public_json::<Value>(Method::GET, "/v1/agreements", None).unwrap_err().is_client_update_required());
        assert_eq!(old.test_request_receipts().len(), before);
        assert!(old.upgrade_latch().begin_ordinary_durable_commit().is_err());
        assert!(old.upgrade_latch().apply_if_open(|| panic!("late ordinary UI mutation")).is_err());
    }
    assert!(!current.upgrade_latch().is_tripped());
    AuthApi::new(current).logout_scoped(false, &current_scope).unwrap();
}

const MOCK_PNG: [u8; 68] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5, 1, 1,
    39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
const TRANSPARENT_MASK_PNG: [u8; 68] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 96, 96, 0, 0, 0, 3, 0, 1, 43,
    9, 77, 132, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
const VALID_UPLOAD_SHA256: &str =
    "431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460";
const MOCK_PASSWORD_CODE: &str = "123456";
const PASSWORD_ALPHA: &str = "CrossAlpha1!";
const PASSWORD_BETA: &str = "CrossBeta2!";
const PASSWORD_RESET: &str = "CrossReset3!";
const OUTDATED_PASSWORD_CLIENT_VERSION: &str = "0.0.0";

fn base_url() -> Url {
    Url::parse(
        &std::env::var("ARTFORGE_CROSS_STACK_BASE_URL")
            .expect("ARTFORGE_CROSS_STACK_BASE_URL is required"),
    )
    .expect("valid Mock API URL")
}

fn mock_code() -> String {
    std::env::var("ARTFORGE_MOCK_EMAIL_CODE").unwrap_or_else(|_| "654321".to_string())
}

fn mock_redemption_code() -> String {
    if let Ok(path) = std::env::var("ARTFORGE_MOCK_REDEMPTION_CODE_FILE") {
        let csv = std::fs::read_to_string(path).expect("read one-time redemption fixture CSV");
        return csv
            .lines()
            .nth(1)
            .and_then(|line| line.split(',').nth(1))
            .map(|value| value.trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
            .expect("fixture CSV contains one redemption code");
    }
    std::env::var("ARTFORGE_MOCK_REDEMPTION_CODE")
        .expect("a redemption code or fixture CSV is required for the cross-stack test")
}

fn new_client_with(device_id: String, app_version: &str) -> ApiClient {
    new_client_identity(
        device_id,
        "Cross-stack test device".to_string(),
        "macos".to_string(),
        app_version,
    )
}

fn new_client_identity(
    device_id: String,
    device_name: String,
    platform: String,
    app_version: &str,
) -> ApiClient {
    ApiClient::new(
        ApiClientConfig {
            base_url: base_url(),
            app_version: app_version.to_string(),
            timeout: Duration::from_secs(10),
        },
        DeviceIdentity {
            id: device_id,
            name: device_name,
            platform,
        },
        Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        ))),
    )
    .expect("create frontend API client")
}

fn new_client() -> ApiClient {
    new_client_with(
        format!("cross-stack-{}", Uuid::new_v4()),
        env!("CARGO_PKG_VERSION"),
    )
}

fn agreement_acceptances(auth: &AuthApi) -> Vec<AgreementAcceptance> {
    auth.list_agreements()
        .expect("load agreements")
        .into_iter()
        .map(|agreement| AgreementAcceptance {
            agreement_type: agreement.agreement_type,
            version: agreement.version,
        })
        .collect()
}

fn login_and_install_authenticated_email(
    client: &ApiClient,
    auth: &AuthApi,
    email: &str,
    code: &str,
    acceptances: &[AgreementAcceptance],
) -> Result<LoginResponse, ApiError> {
    let outcome = auth.login_response(email, code, acceptances)?;
    let EmailLoginOutcome::Authenticated { login } = outcome else {
        panic!("cross-stack authenticated fixture unexpectedly required invited registration");
    };
    client
        .session()
        .install_tokens_for_user(&login.tokens, &login.user.id)?;
    Ok(login)
}

fn login_new_user() -> (ApiClient, LoginResponse) {
    let (client, login, _) = login_new_user_with_email("client-stack");
    (client, login)
}

fn login_new_user_with_email(prefix: &str) -> (ApiClient, LoginResponse, String) {
    let client = new_client();
    let auth = AuthApi::new(client.clone());
    let email = format!("{prefix}-{}@example.com", Uuid::new_v4());
    let delivery = auth
        .request_email_code(&email)
        .expect("request Mock email code");
    assert!(delivery.expires_in_seconds > 0);
    assert!(delivery.resend_after_seconds > 0);
    let login = login_and_install_authenticated_email(
        &client,
        &auth,
        &email,
        &mock_code(),
        &agreement_acceptances(&auth),
    )
        .expect("login through backend");
    assert!(login.is_new_user);
    assert!(login
        .registration_credit_granted
        .parse::<u64>()
        .is_ok_and(|credits| credits > 0));
    (client, login, email)
}

#[test]
#[ignore = "requires the isolated team Mock API with its control token on 127.0.0.1:39091"]
fn cross_stack_selected_account_wire_owner_member_and_frozen() {
    // This fixture mutates only the disposable mock schema. Never accept an
    // arbitrary configured API endpoint or fall back to a normal service.
    assert_eq!(std::env::var("ARTFORGE_ENABLE_MOCK_API").as_deref(), Ok("1"));
    assert_eq!(base_url().as_str(), "http://127.0.0.1:39091/");
    let control_token = std::env::var("ARTFORGE_MOCK_CONTROL_TOKEN").expect("mock control token");
    assert!(control_token.len() == 64
        && control_token.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    let login_fixture = || {
        let client = new_client();
        let auth = AuthApi::new(client.clone());
        let email = format!("account-wire-{}@example.com", Uuid::new_v4());
        auth.request_email_code(&email).expect("request isolated email code");
        let login = login_and_install_authenticated_email(
            &client, &auth, &email, &mock_code(), &agreement_acceptances(&auth),
        ).expect("authenticate isolated account");
        let session = client.session().scope_for_user(&login.user.id).unwrap();
        let group = TeamApi::new(client.clone()).list_groups(&session).unwrap()
            .items.into_iter().find(|group| group.role == "owner").unwrap();
        let scope = BillingScope {
            request: GroupRequestScope { session, account_group_id: group.group_id },
            context_epoch: 1,
        };
        (client, email, scope)
    };
    let (owner, _, owner_scope) = login_fixture();
    let (member, member_email, personal_scope) = login_fixture();
    let owner_snapshot = AccountApi::new(owner.clone()).snapshot_billing(&owner_scope)
        .expect("decode real owner snapshot and authorized sibling responses");
    assert_eq!(owner_snapshot.account.billing_group.role, "owner");
    assert_eq!(owner_snapshot.account.user.id, owner_scope.request.session.owner_user_id);
    assert!(owner_snapshot.account.credits.is_some());
    assert!(owner_snapshot.account.membership.is_some());
    assert!(owner_snapshot.account.quota.is_none());
    assert!(owner_snapshot.owner_billing.is_some());
    assert!(owner_snapshot.orders.is_some());

    let invitation = TeamApi::new(owner.clone()).create_invitation(
        &owner_scope.request.account_group_id, &member_email, "500",
        &Uuid::new_v4().to_string(), &owner_scope.request.session,
    ).expect("invite existing isolated member");
    TeamApi::new(member.clone()).accept_invitation(
        &invitation.invitation_id, &invitation.version,
        &Uuid::new_v4().to_string(), &personal_scope.request.session,
    ).expect("accept team invitation");
    let team_scope = BillingScope {
        request: GroupRequestScope {
            session: personal_scope.request.session.clone(),
            account_group_id: owner_scope.request.account_group_id.clone(),
        },
        context_epoch: 2,
    };
    let team_snapshot = AccountApi::new(member.clone()).snapshot_billing(&team_scope)
        .expect("decode real member snapshot without requesting owner finance");
    assert_eq!(team_snapshot.account.billing_group.role, "member");
    assert_eq!(team_snapshot.account.user.id, personal_scope.request.session.owner_user_id);
    assert_eq!(team_snapshot.account.quota.unwrap().remaining, "500");
    assert!(team_snapshot.account.credits.is_none());
    assert!(team_snapshot.account.membership.is_none());
    assert!(team_snapshot.owner_billing.is_none());
    assert!(team_snapshot.orders.is_none());
    assert!(team_snapshot.plans.is_none());
    assert!(team_snapshot.packs.is_none());
    assert!(team_snapshot.models.is_some());
    let denied = AccountApi::new(member.clone()).credit_account_billing(&team_scope);
    assert!(matches!(denied, Err(ApiError::Http { status: 403, .. })));
    assert!(AccountApi::new(member.clone()).snapshot_billing(&personal_scope)
        .expect("personal group remains independent").account.credits.is_some());

    let frozen = reqwest::blocking::Client::new()
        .post(base_url().join("__mock__/team-account-fixtures").unwrap())
        .timeout(Duration::from_secs(10))
        .header("X-Mock-Control-Token", control_token)
        .json(&json!({"operation": "set_group_status",
            "group_id": owner_scope.request.account_group_id, "status": "frozen"}))
        .send().expect("freeze only the isolated fixture group");
    assert_eq!(frozen.status().as_u16(), 200);
    let frozen_snapshot = AccountApi::new(owner.clone()).snapshot_billing(&owner_scope)
        .expect("frozen owner snapshot remains readable");
    assert!(!frozen_snapshot.account.billing_group.selectable);
    assert!(frozen_snapshot.account.billing_group.readable_context);
    assert!(frozen_snapshot.owner_billing.is_some());
    assert!(frozen_snapshot.models.is_none());
    assert!(frozen_snapshot.orders.is_none());
    assert!(frozen_snapshot.plans.is_none());
    assert!(frozen_snapshot.packs.is_none());
    AuthApi::new(member).logout_scoped(false, &personal_scope.request.session).unwrap();
    AuthApi::new(owner).logout_scoped(false, &owner_scope.request.session).unwrap();
}

fn assert_http_error<T>(result: Result<T, ApiError>, expected_status: u16, expected_code: &str) {
    assert_http_error_field(result, expected_status, expected_code, None);
}

fn assert_trusted_mock_checkout(order: &OrderDetail) {
    let checkout_url = order
        .payment
        .as_ref()
        .and_then(|payment| payment.checkout_url.as_deref())
        .expect("pending payment exposes checkout URL");
    let checkout = reqwest::Url::parse(checkout_url).expect("hosted payment checkout URL");
    assert_eq!(checkout.path(), "/v1/payments/alipay/checkout");
    let fragment = checkout.fragment().expect("payment checkout fragment");
    assert!(fragment.contains("order_id="));
    assert!(fragment.contains("token="));
}

fn assert_http_error_field<T>(
    result: Result<T, ApiError>,
    expected_status: u16,
    expected_code: &str,
    expected_field: Option<&str>,
) {
    match result {
        Err(ApiError::Http {
            status,
            code,
            request_id,
            details,
            ..
        }) => {
            assert_eq!(status, expected_status);
            assert_eq!(code, expected_code);
            assert!(request_id.is_some());
            if let Some(field) = expected_field {
                let fields = details
                    .as_ref()
                    .and_then(Value::as_array)
                    .expect("validation error details array");
                assert!(
                    fields
                        .iter()
                        .any(|detail| detail.get("field").and_then(Value::as_str) == Some(field)),
                    "expected validation detail for {field}, got {details:?}"
                );
            }
        }
        Err(error) => panic!("expected HTTP {expected_status} {expected_code}, got {error:?}"),
        Ok(_) => panic!("expected HTTP {expected_status} {expected_code}, got success"),
    }
}

fn assert_raw_problem(
    response: reqwest::blocking::Response,
    expected_status: u16,
    expected_code: &str,
) -> Value {
    assert_eq!(response.status().as_u16(), expected_status);
    let response_request_id = response
        .headers()
        .get("X-Request-ID")
        .expect("error response X-Request-ID")
        .to_str()
        .expect("ASCII response request ID")
        .to_string();
    let body: Value = response.json().expect("JSON error envelope");
    assert_eq!(body["request_id"], response_request_id);
    assert!(body["data"].is_null());
    assert_eq!(body["error"]["code"], expected_code);
    assert!(body["error"]["message"].is_string());
    body
}

fn prompt_request(request_id: String, prompt: &str) -> CreateGenerationTask {
    CreateGenerationTask {
        client_request_id: request_id,
        task_type: "prompt_optimize".to_string(),
        model_code: "openai_prompt".to_string(),
        prompt: prompt.to_string(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: None,
        target_language: None,
    }
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_happy_path_and_dto_contract() {
    let (client, login) = login_new_user();
    let nickname = login.user.nickname.as_deref().expect("email nickname");
    assert!(nickname.starts_with("client-stack-"));

    let snapshot = AccountApi::new(client.clone())
        .snapshot()
        .expect("deserialize account snapshot");
    assert_eq!(snapshot.account.user.id, login.user.id);
    assert_eq!(snapshot.account.user.nickname.as_deref(), Some(nickname));
    assert_eq!(
        snapshot
            .account
            .credits
            .as_ref()
            .map(|value| value.available.as_str()),
        Some(login.registration_credit_granted.as_str())
    );
    let plans = snapshot.plans.as_deref().expect("owner plans");
    let packs = snapshot.packs.as_deref().expect("owner credit packs");
    let models = snapshot.models.as_deref().expect("owner model catalog");
    assert!(plans.iter().any(|plan| plan.code == "basic"));
    assert!(packs.iter().any(|pack| pack.code == "pack_1000"));
    assert!(models.iter().any(|model| model.code == "openai_prompt"));
    assert_eq!(snapshot.sessions.len(), 1);

    let auth = AuthApi::new(client.clone());
    assert!(!auth.refresh().expect("refresh frontend session").is_empty());
    auth.logout(false).expect("logout frontend session");
    assert!(client.session().access_token().is_none());
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_p0_referral_image_edit_and_style_analysis_contracts() {
    let (inviter_client, _) = login_new_user();
    let inviter_account = AccountApi::new(inviter_client.clone());
    let inviter_dashboard = inviter_account
        .invitation_dashboard()
        .expect("load inviter dashboard");
    assert!(inviter_dashboard.overview.enabled);
    assert_eq!(inviter_dashboard.overview.reward_type, "credits");
    assert_eq!(inviter_dashboard.overview.reward_rate_bps, 1_000);
    let invitation_code = inviter_dashboard
        .overview
        .invitation_code
        .expect("server invitation code");

    let (invitee_client, _) = login_new_user();
    let invitee_account = AccountApi::new(invitee_client.clone());
    assert_eq!(
        invitee_account
            .submit_invitation_code(&invitation_code)
            .expect("bind invitation code")
            .as_deref(),
        Some("邀请码填写成功")
    );
    assert!(
        invitee_account
            .snapshot()
            .expect("reload invitee account")
            .account
            .user
            .invitation_code_submitted
    );
    let inviter_after = inviter_account
        .invitation_dashboard()
        .expect("reload inviter dashboard");
    assert_eq!(inviter_after.overview.invitation_count, 1);
    assert_eq!(inviter_after.users.len(), 1);
    assert!(!inviter_after.users[0]
        .email_masked
        .contains("client-stack-"));

    let path = std::env::temp_dir().join(format!("artforge-p0-contract-{}.png", Uuid::new_v4()));
    std::fs::write(&path, MOCK_PNG).expect("write P0 reference fixture");
    let mask_path =
        std::env::temp_dir().join(format!("artforge-p0-contract-mask-{}.png", Uuid::new_v4()));
    std::fs::write(&mask_path, TRANSPARENT_MASK_PNG).expect("write transparent mask fixture");
    let generation = GenerationApi::new(invitee_client.clone());
    let source_file_id = generation
        .upload_prepared_reference(&path)
        .expect("upload edit source");
    let mask_file_id = generation
        .upload_prepared_reference(&mask_path)
        .expect("upload edit mask");
    let edit_task = generation
        .create_image_edit_task(&CreateImageEditTask {
            client_request_id: format!("edit_{}", Uuid::new_v4().simple()),
            task_type: "image_edit".to_string(),
            model_code: "openai_image".to_string(),
            prompt: "replace the marked pixel".to_string(),
            quality: "1K".to_string(),
            aspect_ratio: "1:1".to_string(),
            source_file_id: source_file_id.clone(),
            mask_file_id: mask_file_id.clone(),
        })
        .expect("create image edit task");
    assert_eq!(edit_task.task_type, "image_edit");
    assert_eq!(edit_task.requested_count, 1);
    assert_eq!(edit_task.request["source_file_id"], source_file_id);
    assert_eq!(edit_task.request["mask_file_id"], mask_file_id);
    generation
        .cancel(&edit_task.id)
        .expect("cancel image edit fixture");

    let style_file_id = generation
        .upload_reference(&path)
        .expect("upload style reference");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&mask_path);
    let style_request = CreateGenerationTask {
        client_request_id: format!("style_{}", Uuid::new_v4().simple()),
        task_type: "image_style_analysis".to_string(),
        model_code: "openai_prompt".to_string(),
        prompt: "describe this visual style".to_string(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: Some(vec![style_file_id.clone()]),
        target_language: None,
    };
    let style_task = generation
        .create_task(&style_request)
        .expect("create image style analysis task");
    assert_eq!(style_task.task_type, "image_style_analysis");
    assert_eq!(style_task.requested_count, 1);
    assert_eq!(
        style_task.request["reference_file_ids"],
        json!([style_file_id])
    );
    let reserved_after_create = invitee_account
        .snapshot()
        .expect("load credits after style task create")
        .account
        .credits
        .expect("style task credit account")
        .reserved;
    let replayed_style_task = generation
        .create_task(&style_request)
        .expect("replay image style analysis task");
    let reserved_after_replay = invitee_account
        .snapshot()
        .expect("load credits after style task replay")
        .account
        .credits
        .expect("style replay credit account")
        .reserved;
    assert_eq!(replayed_style_task.id, style_task.id);
    assert_eq!(reserved_after_replay, reserved_after_create);
    generation
        .cancel(&style_task.id)
        .expect("cancel style analysis fixture");

    AuthApi::new(invitee_client)
        .logout(false)
        .expect("logout invitee P0 fixture");
    AuthApi::new(inviter_client)
        .logout(false)
        .expect("logout inviter P0 fixture");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_auth_validation_and_error_envelopes() {
    let client = new_client();
    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/email/code",
            Some(json!({ "email": "not-an-email", "app_version": env!("CARGO_PKG_VERSION") })),
        ),
        400,
        "validation_failed",
    );
    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/email/code",
            Some(json!({ "email": "valid@example.com", "app_version": "1.0" })),
        ),
        400,
        "validation_failed",
    );
    assert_http_error(
        client.public_json::<Value>(Method::GET, "/v1/account", None),
        401,
        "authentication_required",
    );

    let auth = AuthApi::new(client.clone());
    let email = format!("auth-matrix-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&email).expect("request Mock code");
    assert_http_error(
        login_and_install_authenticated_email(
            &client,
            &auth,
            &email,
            "000000",
            &agreement_acceptances(&auth),
        ),
        400,
        "email_code_invalid",
    );
    let login = login_and_install_authenticated_email(
        &client,
        &auth,
        &email,
        &mock_code(),
        &agreement_acceptances(&auth),
    )
        .expect("correct code remains usable after one failed attempt");
    assert_eq!(login.tokens.token_type, "X-Token");
    auth.logout(false).expect("logout auth validation user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_session_device_binding_and_refresh_replay() {
    let (client, login) = login_new_user();
    let wrong_device = new_client_with(
        format!("wrong-device-{}", Uuid::new_v4()),
        env!("CARGO_PKG_VERSION"),
    );
    wrong_device
        .session()
        .install_tokens_for_user(&login.tokens, &login.user.id)
        .expect("install tokens on wrong test device");
    assert_http_error(
        wrong_device.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_device_mismatch",
    );
    assert!(wrong_device.session().access_token().is_none());

    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/refresh",
            Some(json!({
                "refresh_token": "short",
                "device_id": client.device().id,
                "app_version": client.app_version(),
            })),
        ),
        400,
        "validation_failed",
    );

    let old_refresh = login.tokens.refresh_token.clone();
    AuthApi::new(client.clone())
        .refresh()
        .expect("rotate refresh token");
    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/refresh",
            Some(json!({
                "refresh_token": old_refresh,
                "device_id": client.device().id,
                "app_version": client.app_version(),
            })),
        ),
        401,
        "refresh_token_reused",
    );
    assert_http_error(
        client.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_account_catalog_and_pagination_parameters() {
    let (client, login) = login_new_user();
    let snapshot = AccountApi::new(client.clone())
        .snapshot()
        .expect("load full account snapshot");
    let plans = snapshot.plans.as_deref().expect("owner plans");
    let packs = snapshot.packs.as_deref().expect("owner credit packs");
    let models = snapshot.models.as_deref().expect("owner model catalog");
    let ledger = snapshot.ledger.as_deref().expect("owner credit ledger");
    assert!(plans.len() >= 4);
    assert!(packs.len() >= 4);
    assert!(models.len() >= 2);
    assert!(ledger
        .iter()
        .any(|entry| entry.available_delta == login.registration_credit_granted));

    for path in [
        "/v1/credits/ledger?limit=0",
        "/v1/credits/ledger?limit=101",
        "/v1/credits/ledger?cursor=abc",
        "/v1/notifications?limit=0",
        "/v1/notifications?limit=101",
        "/v1/notifications?cursor=abc",
        "/v1/notifications?unread_only=not-a-boolean",
    ] {
        assert_http_error(
            client.authenticated_json::<Value>(Method::GET, path, None, None),
            400,
            "validation_failed",
        );
    }

    let ledger = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            "/v1/credits/ledger?limit=1",
            None,
            None,
        )
        .expect("valid ledger page");
    assert_eq!(ledger.data.len(), 1);
    assert!(ledger.meta.is_some());
    AuthApi::new(client)
        .logout(false)
        .expect("logout account test");
}

#[test]
#[ignore = "requires the dev Mock API server and an active one-time redemption fixture"]
fn cross_stack_credit_redemption_contract_and_idempotency() {
    let (client, login) = login_new_user();
    let account = AccountApi::new(client.clone());
    let scope = client
        .session()
        .scope_for_user(&login.user.id)
        .expect("bound redemption session scope");
    let before = client
        .authenticated_json::<CreditAccount>(Method::GET, "/v1/credits/account", None, None)
        .expect("load pre-redemption account")
        .data;
    let client_request_id = format!("redemption_{}", Uuid::new_v4().simple());
    let code = mock_redemption_code();

    let first = account
        .redeem_credit_code_scoped(&code, &client_request_id, &scope)
        .expect("redeem through the production endpoint");
    let replay = account
        .redeem_credit_code_scoped(&code, &client_request_id, &scope)
        .expect("replay the same HTTP idempotency key");
    assert_eq!(replay.redemption_id, first.redemption_id);
    assert_eq!(replay.credits_granted, first.credits_granted);
    assert_eq!(replay.account.available, first.account.available);

    let before_available = before
        .available
        .parse::<u128>()
        .expect("pre-redemption available credits");
    let granted = first
        .credits_granted
        .parse::<u128>()
        .expect("redemption grant credits");
    let after_available = first
        .account
        .available
        .parse::<u128>()
        .expect("post-redemption available credits");
    assert_eq!(after_available, before_available + granted);
    assert_eq!(first.account.reserved, before.reserved);

    let business_replay = account
        .redeem_credit_code_scoped(
            &code,
            &format!("redemption_{}", Uuid::new_v4().simple()),
            &scope,
        )
        .expect("same-account business replay");
    assert_eq!(business_replay.redemption_id, first.redemption_id);
    assert_eq!(business_replay.account.available, first.account.available);

    let ledger = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            "/v1/credits/ledger?limit=8",
            None,
            None,
        )
        .expect("load redemption ledger")
        .data;
    assert!(ledger.iter().any(|item| {
        item.business_type == "redemption_code" && item.available_delta == first.credits_granted
    }));

    let (other_client, other_login) = login_new_user();
    let other_scope = other_client
        .session()
        .scope_for_user(&other_login.user.id)
        .expect("other bound session scope");
    assert_http_error(
        AccountApi::new(other_client.clone()).redeem_credit_code_scoped(
            &code,
            &format!("redemption_{}", Uuid::new_v4().simple()),
            &other_scope,
        ),
        409,
        "redemption_code_unavailable",
    );
    AuthApi::new(other_client)
        .logout(false)
        .expect("logout other redemption user");
    AuthApi::new(client)
        .logout(false)
        .expect("logout redemption user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_parameter_matrix_and_idempotency() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let valid_id = || format!("task_{}", Uuid::new_v4().simple());

    let invalid_requests = [
        CreateGenerationTask {
            client_request_id: "short".to_string(),
            ..prompt_request("short".to_string(), "prompt")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            prompt: "".to_string(),
            ..prompt_request(valid_id(), "unused")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "unknown".to_string(),
            ..prompt_request(valid_id(), "prompt")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            quality: Some("1K".to_string()),
            ..prompt_request(valid_id(), "prompt")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "prompt_translate".to_string(),
            ..prompt_request(valid_id(), "translate")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: None,
            count: None,
            aspect_ratio: Some("square".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("8K".to_string()),
            count: Some(1),
            aspect_ratio: Some("square".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("1K".to_string()),
            count: Some(0),
            aspect_ratio: Some("square".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("1K".to_string()),
            count: Some(5),
            aspect_ratio: Some("16:9".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("1K".to_string()),
            count: Some(1),
            aspect_ratio: Some("square".to_string()),
            reference_file_ids: Some((0..9).map(|_| Uuid::new_v4().to_string()).collect()),
            ..prompt_request(valid_id(), "image")
        },
    ];
    for request in invalid_requests {
        assert_http_error(generation.create_task(&request), 400, "validation_failed");
    }

    let unavailable = CreateGenerationTask {
        client_request_id: valid_id(),
        model_code: "missing_model".to_string(),
        ..prompt_request(valid_id(), "prompt")
    };
    assert_http_error(
        generation.create_task(&unavailable),
        409,
        "model_unavailable",
    );

    let image_request = CreateGenerationTask {
        client_request_id: valid_id(),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        quality: Some("1K".to_string()),
        count: Some(4),
        aspect_ratio: Some("landscape".to_string()),
        reference_file_ids: Some(Vec::new()),
        ..prompt_request(valid_id(), "valid image")
    };
    let image_task = generation
        .create_task(&image_request)
        .expect("valid image task");
    assert_eq!(image_task.requested_count, 4);
    generation
        .cancel(&image_task.id)
        .expect("cancel valid image task");

    let request_id = valid_id();
    let original = prompt_request(request_id.clone(), "same prompt");
    let first = generation
        .create_task(&original)
        .expect("create prompt task");
    let replay = generation
        .create_task(&original)
        .expect("replay prompt task");
    assert_eq!(replay.id, first.id);
    let conflict = prompt_request(request_id, "different prompt");
    assert_http_error(
        generation.create_task(&conflict),
        409,
        "idempotency_key_conflict",
    );
    assert_http_error(
        generation.list_tasks("invalid-status"),
        400,
        "validation_failed",
    );
    assert_http_error(
        generation.task(&Uuid::new_v4().to_string()),
        404,
        "generation_task_not_found",
    );
    generation.cancel(&first.id).expect("cancel prompt task");
    AuthApi::new(client)
        .logout(false)
        .expect("logout generation test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_payment_parameter_matrix_and_idempotency() {
    let (client, _) = login_new_user();
    let payment = PaymentApi::new(client.clone());
    let packs = payment.packs().expect("load packs");
    let first_pack = packs.first().expect("at least one pack");
    let other_pack = packs
        .iter()
        .find(|pack| pack.code != first_pack.code)
        .expect("at least two packs");

    assert_http_error(
        payment.create_credit_order("BAD-PACK", &format!("credit_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
    );
    assert_http_error(
        payment.create_credit_order(
            "missing_pack",
            &format!("credit_{}", Uuid::new_v4().simple()),
        ),
        404,
        "credit_pack_unavailable",
    );
    assert_http_error(
        payment.create_credit_order(&first_pack.code, "short"),
        400,
        "validation_failed",
    );

    let request_id = format!("credit_{}", Uuid::new_v4().simple());
    let order = payment
        .create_credit_order(&first_pack.code, &request_id)
        .expect("create credit order");
    assert_eq!(order.status, "pending_payment");
    assert_trusted_mock_checkout(&order);
    assert!(order.payable_amount_cents.parse::<u64>().is_ok());
    assert_eq!(
        payment
            .create_credit_order(&first_pack.code, &request_id)
            .expect("replay credit order")
            .id,
        order.id
    );
    assert_http_error(
        payment.create_credit_order(&other_pack.code, &request_id),
        409,
        "idempotency_key_conflict",
    );
    assert_eq!(
        payment
            .sync_order(&order.id)
            .expect("sync pending order")
            .status,
        "pending_payment"
    );
    assert_http_error(
        payment.order(&Uuid::new_v4().to_string()),
        404,
        "order_not_found",
    );

    let membership = MembershipApi::new(client.clone());
    assert_http_error(
        membership.create_order("BAD", &format!("member_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
    );
    assert_http_error(
        membership.create_order(
            "missing_plan",
            &format!("member_{}", Uuid::new_v4().simple()),
        ),
        404,
        "membership_plan_unavailable",
    );
    let membership_request_id = format!("member_{}", Uuid::new_v4().simple());
    let membership_order = membership
        .create_order("basic", &membership_request_id)
        .expect("create membership order");
    assert_eq!(membership_order.status, "pending_payment");
    assert_trusted_mock_checkout(&membership_order);
    assert_eq!(
        membership
            .create_order("basic", &membership_request_id)
            .expect("replay membership order")
            .id,
        membership_order.id
    );
    assert_http_error(
        membership.create_upgrade_quote("advanced"),
        409,
        "membership_missing",
    );
    AuthApi::new(client)
        .logout(false)
        .expect("logout payment test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_reference_upload_and_notification_parameters() {
    let (client, _) = login_new_user();
    for body in [
        json!({ "filename": "", "mime_type": "image/png", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
        json!({ "filename": "test.gif", "mime_type": "image/gif", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
        json!({ "filename": "test.png", "mime_type": "image/png", "size_bytes": 0, "sha256": VALID_UPLOAD_SHA256 }),
        json!({ "filename": "test.png", "mime_type": "image/png", "size_bytes": 68, "sha256": "invalid" }),
    ] {
        assert_http_error(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            ),
            400,
            "validation_failed",
        );
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "large.png",
                "mime_type": "image/png",
                "size_bytes": 20_971_521,
                "sha256": VALID_UPLOAD_SHA256,
            })),
            None,
        ),
        413,
        "reference_image_too_large",
    );

    let path = std::env::temp_dir().join(format!("artforge-cross-stack-{}.png", Uuid::new_v4()));
    std::fs::write(&path, MOCK_PNG).expect("write Mock reference image");
    let generation = GenerationApi::new(client.clone());
    let file_id = generation
        .upload_reference(&path)
        .expect("multipart reference upload");
    let _ = std::fs::remove_file(&path);
    generation.delete_reference(&file_id);

    let notifications = NotificationsApi::new(client.clone());
    assert!(notifications
        .list()
        .expect("empty notifications")
        .is_empty());
    notifications.mark_all_read().expect("mark empty list read");
    assert_http_error(
        notifications.mark_read(&Uuid::new_v4().to_string()),
        404,
        "notification_not_found",
    );
    AuthApi::new(client)
        .logout(false)
        .expect("logout upload test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_email_code_fields_report_exact_validation_details() {
    let client = new_client();
    for (body, field) in [
        (
            json!({ "email": "not-an-email", "app_version": env!("CARGO_PKG_VERSION") }),
            "email",
        ),
        (
            json!({ "email": format!("{}@example.com", "a".repeat(250)), "app_version": env!("CARGO_PKG_VERSION") }),
            "email",
        ),
        (
            json!({ "email": "valid@example.com", "app_version": "1.0" }),
            "app_version",
        ),
        (json!({ "email": "valid@example.com" }), "app_version"),
    ] {
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/email/code", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_login_fields_report_exact_validation_details() {
    let client = new_client();
    let auth = AuthApi::new(client.clone());
    let email = format!("login-fields-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&email)
        .expect("request login field code");
    let device_id = format!("device-{}", Uuid::new_v4());
    let valid_body = || {
        json!({
            "email": email,
            "code": mock_code(),
            "device_id": device_id,
            "device_name": "field test",
            "platform": "macos",
            "app_version": env!("CARGO_PKG_VERSION"),
            "agreement_acceptances": [],
        })
    };
    let mut cases = Vec::new();
    let mut body = valid_body();
    body["code"] = json!("12345");
    cases.push((body, "code"));
    let mut body = valid_body();
    body["device_id"] = json!("short");
    cases.push((body, "device_id"));
    let mut body = valid_body();
    body["device_name"] = json!("x".repeat(129));
    cases.push((body, "device_name"));
    let mut body = valid_body();
    body["platform"] = json!("linux");
    cases.push((body, "platform"));
    let mut body = valid_body();
    body["app_version"] = json!("0.1");
    cases.push((body, "app_version"));
    for (body, field) in cases {
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/email/login", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }
    login_and_install_authenticated_email(&client, &auth, &email, &mock_code(), &[])
        .expect("valid login after validation cases");
    auth.logout(false).expect("logout login field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_agreement_acceptance_fields_and_duplicates() {
    let (client, _) = login_new_user();
    assert_http_error_field(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(json!({ "agreements": [] })),
            None,
        ),
        400,
        "validation_failed",
        Some("agreements"),
    );
    assert_http_error_field(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(json!({ "agreements": [{ "type": "unknown", "version": "1" }] })),
            None,
        ),
        400,
        "validation_failed",
        Some("agreements.0.type"),
    );
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(json!({
                "agreements": [
                    { "type": "user_terms", "version": "1" },
                    { "type": "user_terms", "version": "1" }
                ]
            })),
            None,
        ),
        400,
        "validation_failed",
    );
    AuthApi::new(client)
        .logout(false)
        .expect("logout agreement test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_prompt_task_fields_report_exact_validation_details() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let task_id = || format!("prompt_{}", Uuid::new_v4().simple());
    let cases = [
        (
            CreateGenerationTask {
                client_request_id: "1234567".to_string(),
                ..prompt_request(task_id(), "prompt")
            },
            "client_request_id",
        ),
        (
            CreateGenerationTask {
                client_request_id: "x".repeat(65),
                ..prompt_request(task_id(), "prompt")
            },
            "client_request_id",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                model_code: "OpenAI".to_string(),
                ..prompt_request(task_id(), "prompt")
            },
            "model_code",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "unknown".to_string(),
                ..prompt_request(task_id(), "prompt")
            },
            "task_type",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                prompt: "".to_string(),
                ..prompt_request(task_id(), "unused")
            },
            "prompt",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                prompt: "p".repeat(10_001),
                ..prompt_request(task_id(), "unused")
            },
            "prompt",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                quality: Some("1K".to_string()),
                ..prompt_request(task_id(), "prompt")
            },
            "quality",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "prompt_translate".to_string(),
                ..prompt_request(task_id(), "translate")
            },
            "target_language",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "prompt_translate".to_string(),
                target_language: Some("x".to_string()),
                ..prompt_request(task_id(), "translate")
            },
            "target_language",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "prompt_translate".to_string(),
                target_language: Some("x".repeat(65)),
                ..prompt_request(task_id(), "translate")
            },
            "target_language",
        ),
    ];
    for (request, field) in cases {
        assert_http_error_field(
            generation.create_task(&request),
            400,
            "validation_failed",
            Some(field),
        );
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout prompt field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_image_task_fields_report_exact_validation_details() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let task_id = || format!("image_{}", Uuid::new_v4().simple());
    let image_request = || CreateGenerationTask {
        client_request_id: task_id(),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: "image".to_string(),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(Vec::new()),
        target_language: None,
    };
    let mut cases = Vec::new();
    let mut request = image_request();
    request.quality = None;
    cases.push((request, "quality"));
    let mut request = image_request();
    request.count = None;
    cases.push((request, "count"));
    let mut request = image_request();
    request.quality = Some("8K".to_string());
    cases.push((request, "quality"));
    let mut request = image_request();
    request.count = Some(0);
    cases.push((request, "count"));
    let mut request = image_request();
    request.count = Some(5);
    cases.push((request, "count"));
    let mut request = image_request();
    request.aspect_ratio = Some("7:5".to_string());
    cases.push((request, "aspect_ratio"));
    let mut request = image_request();
    request.reference_file_ids = Some((0..9).map(|_| Uuid::new_v4().to_string()).collect());
    cases.push((request, "reference_file_ids"));
    let mut request = image_request();
    request.reference_file_ids = Some(vec!["not-a-uuid".to_string()]);
    cases.push((request, "reference_file_ids.0"));
    for (request, field) in cases {
        assert_http_error_field(
            generation.create_task(&request),
            400,
            "validation_failed",
            Some(field),
        );
    }
    let duplicate = Uuid::new_v4().to_string();
    let mut request = image_request();
    request.reference_file_ids = Some(vec![duplicate.clone(), duplicate]);
    assert_http_error(generation.create_task(&request), 400, "validation_failed");
    let mut request = image_request();
    request.quality = Some("2K".to_string());
    let universal_quality_task = generation
        .create_task(&request)
        .expect("all memberships can use 2K quality");
    generation
        .cancel(&universal_quality_task.id)
        .expect("cancel universal quality fixture");
    AuthApi::new(client)
        .logout(false)
        .expect("logout image field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_success_variants_and_credit_reservation_limit() {
    let (client, login) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let translate = CreateGenerationTask {
        client_request_id: format!("translate_{}", Uuid::new_v4().simple()),
        task_type: "prompt_translate".to_string(),
        model_code: "openai_prompt".to_string(),
        prompt: "translate this".to_string(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: None,
        target_language: Some("English".to_string()),
    };
    let translate_task = generation
        .create_task(&translate)
        .expect("valid translate task");
    generation
        .cancel(&translate_task.id)
        .expect("cancel translate task");

    for (ratio, normalized, width, height) in [
        ("1:1", "1:1", 1024, 1024),
        ("3:2", "3:2", 1024, 680),
        ("2:3", "2:3", 680, 1024),
        ("4:3", "4:3", 1024, 768),
        ("3:4", "3:4", 768, 1024),
        ("5:4", "5:4", 1024, 816),
        ("4:5", "4:5", 816, 1024),
        ("16:9", "16:9", 1024, 576),
        ("9:16", "9:16", 576, 1024),
        ("2:1", "2:1", 1024, 512),
        ("1:2", "1:2", 512, 1024),
        ("21:9", "21:9", 1024, 440),
        ("9:21", "9:21", 440, 1024),
        ("square", "1:1", 1024, 1024),
        ("landscape", "3:2", 1024, 680),
        ("portrait", "2:3", 680, 1024),
    ] {
        let request = CreateGenerationTask {
            client_request_id: format!("ratio_{}", Uuid::new_v4().simple()),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            prompt: format!("valid {ratio} image"),
            quality: Some("1K".to_string()),
            count: Some(1),
            aspect_ratio: Some(ratio.to_string()),
            reference_file_ids: Some(Vec::new()),
            target_language: None,
        };
        let task = generation.create_task(&request).expect("valid ratio task");
        assert_eq!(task.request["aspect_ratio"], normalized);
        assert_eq!(task.request["target_width"], width);
        assert_eq!(task.request["target_height"], height);
        assert_eq!(task.request["provider_size"], format!("{width}x{height}"));
        generation.cancel(&task.id).expect("cancel ratio task");
    }

    let four_images = |label: &str| CreateGenerationTask {
        client_request_id: format!("{label}_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: label.to_string(),
        quality: Some("1K".to_string()),
        count: Some(4),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(Vec::new()),
        target_language: None,
    };
    let first = generation
        .create_task(&four_images("reserve_a"))
        .expect("reserve 200 credits A");
    let second = generation
        .create_task(&four_images("reserve_b"))
        .expect("reserve 200 credits B");
    assert_http_error(
        generation.create_task(&four_images("reserve_c")),
        409,
        "insufficient_credits",
    );
    generation.cancel(&first.id).expect("release reservation A");
    generation
        .cancel(&second.id)
        .expect("release reservation B");
    let credits = client
        .authenticated_json::<CreditAccount>(Method::GET, "/v1/credits/account", None, None)
        .expect("load credits after cancellation")
        .data;
    assert_eq!(credits.available, login.registration_credit_granted);
    assert_eq!(credits.reserved, "0");
    AuthApi::new(client)
        .logout(false)
        .expect("logout generation success test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_queue_limit_is_exactly_twenty() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let mut task_ids = Vec::new();
    for index in 0..20 {
        let request = prompt_request(
            format!("queue_{index}_{}", Uuid::new_v4().simple()),
            &format!("queued prompt {index}"),
        );
        task_ids.push(generation.create_task(&request).expect("queue task").id);
    }
    assert_eq!(
        generation
            .list_tasks("queued")
            .expect("list queued tasks")
            .len(),
        20
    );
    assert_http_error(
        generation.create_task(&prompt_request(
            format!("queue_over_{}", Uuid::new_v4().simple()),
            "one task too many",
        )),
        429,
        "generation_queue_limit_reached",
    );
    for task_id in task_ids {
        generation.cancel(&task_id).expect("cancel queued task");
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout queue limit test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_order_request_id_boundaries_report_exact_fields() {
    let (client, _) = login_new_user();
    let payment = PaymentApi::new(client.clone());
    for request_id in ["1234567".to_string(), "x".repeat(65)] {
        assert_http_error_field(
            payment.create_credit_order("pack_1000", &request_id),
            400,
            "validation_failed",
            Some("client_request_id"),
        );
        assert_http_error_field(
            MembershipApi::new(client.clone()).create_order("basic", &request_id),
            400,
            "validation_failed",
            Some("client_request_id"),
        );
    }
    assert_http_error_field(
        payment.create_credit_order("BAD-PACK", &format!("credit_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
        Some("pack_code"),
    );
    assert_http_error_field(
        MembershipApi::new(client.clone())
            .create_order("BAD-PLAN", &format!("member_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
        Some("plan_code"),
    );
    AuthApi::new(client)
        .logout(false)
        .expect("logout order boundary test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_upgrade_and_order_identifier_boundaries() {
    let (client, _) = login_new_user();
    let membership = MembershipApi::new(client.clone());
    assert_http_error_field(
        membership.create_upgrade_quote("BAD-PLAN"),
        400,
        "validation_failed",
        Some("target_plan_code"),
    );
    assert_http_error(
        membership.create_upgrade_quote("free"),
        404,
        "membership_plan_unavailable",
    );
    assert_http_error_field(
        membership.create_upgrade_order("not-a-uuid", "upgrade_12345678"),
        400,
        "validation_failed",
        Some("quote_id"),
    );
    assert_http_error(
        membership.create_upgrade_order(&Uuid::new_v4().to_string(), "upgrade_12345678"),
        409,
        "upgrade_quote_unavailable",
    );
    assert_http_error(
        client.authenticated_json::<Value>(Method::GET, "/v1/orders/not-a-uuid", None, None),
        400,
        "validation_failed",
    );
    assert_http_error(
        AccountApi::new(client.clone()).revoke_session(&Uuid::new_v4().to_string()),
        404,
        "session_not_found",
    );
    AuthApi::new(client)
        .logout(false)
        .expect("logout identifier test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_upload_filename_and_size_boundaries_report_exact_fields() {
    let (client, _) = login_new_user();
    let cases = [
        (
            json!({ "filename": "", "mime_type": "image/png", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
            "filename",
        ),
        (
            json!({ "filename": "x".repeat(256), "mime_type": "image/png", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
            "filename",
        ),
        (
            json!({ "filename": "test.gif", "mime_type": "image/gif", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
            "mime_type",
        ),
        (
            json!({ "filename": "test.png", "mime_type": "image/png", "size_bytes": 0, "sha256": VALID_UPLOAD_SHA256 }),
            "size_bytes",
        ),
        (
            json!({ "filename": "test.png", "mime_type": "image/png", "size_bytes": 68, "sha256": "invalid" }),
            "sha256",
        ),
    ];
    for (body, field) in cases {
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    let legacy_upload = client
        .authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "legacy.png",
                "mime_type": "image/png",
                "size_bytes": 68,
            })),
            None,
        )
        .expect("legacy upload may omit sha256")
        .data;
    let legacy_file_id = legacy_upload["file"]["id"]
        .as_str()
        .expect("legacy pending file id");
    GenerationApi::new(client.clone()).delete_reference(legacy_file_id);
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "large.png",
                "mime_type": "image/png",
                "size_bytes": 20_971_521,
                "sha256": VALID_UPLOAD_SHA256,
            })),
            None,
        ),
        413,
        "reference_image_too_large",
    );
    AuthApi::new(client)
        .logout(false)
        .expect("logout upload boundary test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_auth_required_fields_and_minimum_accepted_boundaries() {
    let client = new_client();
    assert_http_error_field(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/email/code",
            Some(json!({ "app_version": env!("CARGO_PKG_VERSION") })),
        ),
        400,
        "validation_failed",
        Some("email"),
    );

    let email = format!("required-fields-{}@example.com", Uuid::new_v4());
    let auth = AuthApi::new(client.clone());
    auth.request_email_code(&email)
        .expect("request required field code");
    let valid_login = json!({
        "email": email,
        "code": mock_code(),
        "device_id": "12345678",
        "device_name": "",
        "platform": "windows",
        "app_version": env!("CARGO_PKG_VERSION"),
        "agreement_acceptances": agreement_acceptances(&auth),
    });
    for field in ["email", "code", "device_id", "platform", "app_version"] {
        let mut body = valid_login.clone();
        body.as_object_mut().expect("login object").remove(field);
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/email/login", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }

    let min_client = new_client_identity(
        "12345678".to_string(),
        String::new(),
        "windows".to_string(),
        env!("CARGO_PKG_VERSION"),
    );
    let min_auth = AuthApi::new(min_client.clone());
    login_and_install_authenticated_email(
        &min_client,
        &min_auth,
        &email,
        &mock_code(),
        &agreement_acceptances(&min_auth),
    )
        .expect("minimum device and empty optional name are accepted");
    let raw_sessions = min_client
        .authenticated_json::<Value>(Method::GET, "/v1/account/sessions", None, None)
        .expect("load raw minimum boundary sessions")
        .data;
    assert!(
        raw_sessions["items"][0]["device_name"].is_string(),
        "account session device_name must follow the OpenAPI string contract, got {}",
        raw_sessions["items"][0]["device_name"]
    );
    let min_session = AccountApi::new(min_client.clone())
        .snapshot()
        .expect("minimum boundary snapshot")
        .sessions
        .into_iter()
        .find(|session| session.is_current)
        .expect("current minimum boundary session");
    assert_eq!(min_session.device_name, "");
    assert_eq!(min_session.platform, "windows");
    AuthApi::new(min_client)
        .logout(false)
        .expect("logout minimum boundary user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_auth_maximum_boundaries_and_email_normalization() {
    let max_client = new_client_identity(
        "d".repeat(256),
        "n".repeat(128),
        "macos".to_string(),
        env!("CARGO_PKG_VERSION"),
    );
    let max_auth = AuthApi::new(max_client.clone());
    let normalized_email = format!("  Boundary-{}@Example.COM  ", Uuid::new_v4());
    max_auth
        .request_email_code(&normalized_email)
        .expect("request normalized email code");
    login_and_install_authenticated_email(
        &max_client,
        &max_auth,
        &normalized_email,
        &mock_code(),
        &agreement_acceptances(&max_auth),
    )
        .expect("maximum device and name lengths are accepted");
    let max_session = AccountApi::new(max_client.clone())
        .snapshot()
        .expect("maximum boundary snapshot")
        .sessions
        .into_iter()
        .find(|session| session.is_current)
        .expect("current maximum boundary session");
    assert_eq!(max_session.device_name.chars().count(), 128);
    assert_eq!(max_session.platform, "macos");
    AuthApi::new(max_client)
        .logout(false)
        .expect("logout maximum boundary user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_refresh_required_fields_and_authenticated_client_version() {
    let (client, login) = login_new_user();
    let valid_refresh = json!({
        "refresh_token": login.tokens.refresh_token,
        "device_id": client.device().id,
        "app_version": client.app_version(),
    });
    for field in ["refresh_token", "device_id", "app_version"] {
        let mut body = valid_refresh.clone();
        body.as_object_mut().expect("refresh object").remove(field);
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/refresh", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }
    for (value, field) in [
        (json!("1234567"), "device_id"),
        (json!("d".repeat(257)), "device_id"),
        (json!("1.0"), "app_version"),
    ] {
        let mut body = valid_refresh.clone();
        body[field] = value;
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/refresh", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }

    let invalid_version = new_client_with(client.device().id.clone(), "1.0");
    invalid_version
        .session()
        .install_tokens_for_user(&login.tokens, &login.user.id)
        .expect("install tokens for malformed version test");
    assert_http_error(
        invalid_version.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        400,
        "client_version_invalid",
    );
    let dev_minimum = new_client_with(client.device().id.clone(), "0.0.0");
    dev_minimum
        .session()
        .install_tokens_for_user(&login.tokens, &login.user.id)
        .expect("install tokens for dev minimum version test");
    assert!(!dev_minimum
        .authenticated_json::<Value>(Method::GET, "/v1/account", None, None)
        .expect("dev minimum client version is accepted")
        .request_id
        .is_empty());
    AuthApi::new(client)
        .logout(false)
        .expect("logout refresh field user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_email_code_cooldown_and_attempt_exhaustion() {
    let client = new_client();
    let auth = AuthApi::new(client.clone());
    let cooldown_email = format!("cooldown-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&cooldown_email)
        .expect("first code request");
    assert_http_error(
        auth.request_email_code(&cooldown_email),
        429,
        "email_code_cooldown",
    );

    let attempts_email = format!("attempts-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&attempts_email)
        .expect("request attempt limit code");
    let agreements = agreement_acceptances(&auth);
    for _ in 0..4 {
        assert_http_error(
            login_and_install_authenticated_email(
                &client,
                &auth,
                &attempts_email,
                "000000",
                &agreements,
            ),
            400,
            "email_code_invalid",
        );
    }
    assert_http_error(
        login_and_install_authenticated_email(
            &client,
            &auth,
            &attempts_email,
            "000000",
            &agreements,
        ),
        400,
        "email_code_attempts_exceeded",
    );
    assert_http_error(
        login_and_install_authenticated_email(
            &client,
            &auth,
            &attempts_email,
            &mock_code(),
            &agreements,
        ),
        400,
        "email_code_invalid",
    );
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_agreement_item_required_fields_and_version_boundaries() {
    let (client, _) = login_new_user();
    for (agreement, field) in [
        (json!({ "version": "1" }), "agreements.0.type"),
        (json!({ "type": "user_terms" }), "agreements.0.version"),
        (
            json!({ "type": "user_terms", "version": "" }),
            "agreements.0.version",
        ),
        (
            json!({ "type": "user_terms", "version": "v".repeat(33) }),
            "agreements.0.version",
        ),
    ] {
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/agreements/accept",
                Some(json!({ "agreements": [agreement] })),
                None,
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    let current = agreement_acceptances(&AuthApi::new(client.clone()));
    AuthApi::new(client.clone())
        .accept_agreements(&current)
        .expect("re-accept current agreements once");
    AuthApi::new(client.clone())
        .accept_agreements(&current)
        .expect("re-accept current agreements idempotently");
    AuthApi::new(client)
        .logout(false)
        .expect("logout agreement item test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_required_fields_and_forbidden_combinations() {
    let (client, _) = login_new_user();
    let valid = serde_json::to_value(prompt_request("required1".to_string(), "prompt"))
        .expect("serialize valid prompt request");
    for field in ["client_request_id", "task_type", "model_code", "prompt"] {
        let mut body = valid.clone();
        body.as_object_mut()
            .expect("task body object")
            .remove(field);
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some("required_header_1"),
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    for (field, value) in [
        ("count", json!(1)),
        ("aspect_ratio", json!("square")),
        ("reference_file_ids", json!([])),
        ("target_language", json!("English")),
    ] {
        let mut body = valid.clone();
        body[field] = value;
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some("forbidden_header_1"),
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout generation required field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_exact_valid_boundaries_defaults_and_header_idempotency() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let boundary_requests = [
        prompt_request("12345678".to_string(), "p"),
        prompt_request("x".repeat(64), &"p".repeat(10_000)),
        CreateGenerationTask {
            client_request_id: format!("language_min_{}", Uuid::new_v4().simple()),
            task_type: "prompt_translate".to_string(),
            model_code: "openai_prompt".to_string(),
            prompt: "translate".to_string(),
            quality: None,
            count: None,
            aspect_ratio: None,
            reference_file_ids: None,
            target_language: Some("zh".to_string()),
        },
        CreateGenerationTask {
            client_request_id: format!("language_max_{}", Uuid::new_v4().simple()),
            task_type: "prompt_translate".to_string(),
            model_code: "openai_prompt".to_string(),
            prompt: "translate".to_string(),
            quality: None,
            count: None,
            aspect_ratio: None,
            reference_file_ids: None,
            target_language: Some("l".repeat(64)),
        },
    ];
    let mut task_ids = Vec::new();
    for request in boundary_requests {
        task_ids.push(
            generation
                .create_task(&request)
                .expect("accepted generation boundary")
                .id,
        );
    }
    let image_default = json!({
        "client_request_id": format!("defaults_{}", Uuid::new_v4().simple()),
        "task_type": "image_generation",
        "model_code": "openai_image",
        "prompt": "default image fields",
        "quality": "1K",
        "count": 1,
    });
    let default_task = client
        .authenticated_json::<GenerationTaskDetail>(
            Method::POST,
            "/v1/generation/tasks",
            Some(image_default.clone()),
            Some(
                image_default["client_request_id"]
                    .as_str()
                    .expect("default request id"),
            ),
        )
        .expect("image defaults are accepted")
        .data;
    assert_eq!(default_task.request["aspect_ratio"], "1:1");
    assert_eq!(default_task.request["provider_size"], "1024x1024");
    assert_eq!(default_task.request["reference_file_ids"], json!([]));
    task_ids.push(default_task.id);

    let header_body = serde_json::to_value(prompt_request(
        format!("header_body_{}", Uuid::new_v4().simple()),
        "header boundary",
    ))
    .expect("serialize header task");
    for header in ["1234567".to_string(), "h".repeat(129)] {
        assert_http_error(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(header_body.clone()),
                Some(&header),
            ),
            400,
            "idempotency_key_required",
        );
    }
    let header_task = client
        .authenticated_json::<GenerationTaskDetail>(
            Method::POST,
            "/v1/generation/tasks",
            Some(header_body),
            Some(&"h".repeat(128)),
        )
        .expect("128 character idempotency header is accepted")
        .data;
    task_ids.push(header_task.id);
    for task_id in task_ids {
        generation
            .cancel(&task_id)
            .expect("cancel accepted boundary task");
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout generation boundary test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_list_cancel_purge_and_delivery_state_matrix() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let task = generation
        .create_task(&prompt_request(
            format!("state_{}", Uuid::new_v4().simple()),
            "state matrix",
        ))
        .expect("create state task");
    for query in [
        "limit=1",
        "limit=100",
        "limit=20&cursor=0",
        "status=queued",
        "status=processing",
        "status=completed",
        "status=partially_completed",
        "status=failed",
        "status=cancelled",
    ] {
        assert!(!client
            .authenticated_json::<Value>(
                Method::GET,
                &format!("/v1/generation/tasks?{query}"),
                None,
                None,
            )
            .expect("valid generation list query")
            .request_id
            .is_empty());
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/generation/tasks/{}/content", task.id),
            None,
            None,
        ),
        409,
        "generation_task_active",
    );
    generation.cancel(&task.id).expect("first cancellation");
    generation
        .cancel(&task.id)
        .expect("idempotent second cancellation");
    let cancelled = generation.task(&task.id).expect("load cancelled task");
    assert_eq!(cancelled.status, "cancelled");
    for _ in 0..2 {
        let purged = client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/generation/tasks/{}/content", task.id),
                None,
                None,
            )
            .expect("content purge is idempotent")
            .data;
        assert_eq!(purged["content_status"], "deleted");
    }

    let ack_path = format!(
        "/v1/generation/tasks/{}/deliveries/{}/ack",
        task.id,
        Uuid::new_v4()
    );
    for (body, field) in [
        (json!({ "size_bytes": 1 }), "sha256"),
        (json!({ "sha256": "0".repeat(64) }), "size_bytes"),
        (json!({ "sha256": "bad", "size_bytes": 1 }), "sha256"),
        (
            json!({ "sha256": "0".repeat(64), "size_bytes": 0 }),
            "size_bytes",
        ),
    ] {
        assert_http_error_field(
            client.authenticated_json::<Value>(Method::POST, &ack_path, Some(body), None),
            400,
            "validation_failed",
            Some(field),
        );
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            &ack_path,
            Some(json!({ "sha256": "a".repeat(64), "size_bytes": 1 })),
            None,
        ),
        404,
        "result_file_not_found",
    );
    for method_path in [
        (Method::GET, "/v1/generation/tasks/not-a-uuid".to_string()),
        (
            Method::POST,
            "/v1/generation/tasks/not-a-uuid/cancel".to_string(),
        ),
        (
            Method::DELETE,
            "/v1/generation/tasks/not-a-uuid/content".to_string(),
        ),
    ] {
        assert_http_error(
            client.authenticated_json::<Value>(method_path.0, &method_path.1, None, None),
            404,
            "generation_task_not_found",
        );
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout generation state test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_upload_accepted_boundaries_repeated_states_and_pending_limit() {
    let (client, _) = login_new_user();
    let prepare = |body: Value| {
        client
            .authenticated_json::<Value>(Method::POST, "/v1/uploads/references", Some(body), None)
            .expect("prepare accepted reference")
            .data["file"]["id"]
            .as_str()
            .expect("prepared file id")
            .to_string()
    };
    for body in [
        json!({ "filename": "x", "mime_type": "image/png", "size_bytes": 1, "sha256": VALID_UPLOAD_SHA256 }),
        json!({ "filename": "x".repeat(255), "mime_type": "image/png", "size_bytes": 10_485_760, "sha256": VALID_UPLOAD_SHA256 }),
        json!({ "filename": "a.jpg", "mime_type": "image/jpeg", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
        json!({ "filename": "a.webp", "mime_type": "image/webp", "size_bytes": 68, "sha256": VALID_UPLOAD_SHA256 }),
    ] {
        let file_id = prepare(body);
        client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/uploads/references/{file_id}"),
                None,
                None,
            )
            .expect("delete accepted reference boundary");
    }

    let file_id = prepare(json!({
        "filename": "state.png",
        "mime_type": "image/png",
        "size_bytes": 68,
        "sha256": VALID_UPLOAD_SHA256,
    }));
    for _ in 0..2 {
        let completed = client
            .authenticated_json::<Value>(
                Method::POST,
                &format!("/v1/uploads/references/{file_id}/complete"),
                None,
                None,
            )
            .expect("reference completion is idempotent")
            .data;
        assert_eq!(completed["status"], "uploaded");
    }
    for _ in 0..2 {
        let deleted = client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/uploads/references/{file_id}"),
                None,
                None,
            )
            .expect("reference deletion is idempotent")
            .data;
        assert_eq!(deleted["status"], "deleted");
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/uploads/references/{file_id}/complete"),
            None,
            None,
        ),
        409,
        "reference_upload_unavailable",
    );

    let mut pending = Vec::new();
    for index in 0..32 {
        pending.push(prepare(json!({
            "filename": format!("pending-{index}.png"),
            "mime_type": "image/png",
            "size_bytes": 1,
            "sha256": VALID_UPLOAD_SHA256,
        })));
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "pending-overflow.png",
                "mime_type": "image/png",
                "size_bytes": 1,
                "sha256": VALID_UPLOAD_SHA256,
            })),
            None,
        ),
        429,
        "reference_upload_limit_reached",
    );
    for pending_id in pending {
        client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/uploads/references/{pending_id}"),
                None,
                None,
            )
            .expect("delete pending limit fixture");
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout upload state test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_cross_user_resource_and_idempotency_isolation() {
    let (client_a, _) = login_new_user();
    let (client_b, _) = login_new_user();
    let generation_a = GenerationApi::new(client_a.clone());
    let generation_b = GenerationApi::new(client_b.clone());
    let shared_request_id = format!("shared_{}", Uuid::new_v4().simple());
    let task_a = generation_a
        .create_task(&prompt_request(shared_request_id.clone(), "user A"))
        .expect("create user A task");
    let task_b = generation_b
        .create_task(&prompt_request(shared_request_id, "user B"))
        .expect("same idempotency key is isolated by user");
    assert_ne!(task_a.id, task_b.id);
    assert_http_error(
        generation_b.task(&task_a.id),
        404,
        "generation_task_not_found",
    );
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/generation/tasks/{}/cancel", task_a.id),
            None,
            None,
        ),
        404,
        "generation_task_not_found",
    );
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/generation/tasks/{}/content", task_a.id),
            None,
            None,
        ),
        404,
        "generation_task_not_found",
    );

    let session_a = AccountApi::new(client_a.clone())
        .snapshot()
        .expect("load user A session")
        .sessions[0]
        .id
        .clone();
    assert_http_error(
        AccountApi::new(client_b.clone()).revoke_session(&session_a),
        404,
        "session_not_found",
    );

    let order_a = PaymentApi::new(client_a.clone())
        .create_credit_order("pack_1000", &format!("order_a_{}", Uuid::new_v4().simple()))
        .expect("create user A order");
    assert_http_error(
        PaymentApi::new(client_b.clone()).order(&order_a.id),
        404,
        "order_not_found",
    );
    assert_http_error(
        PaymentApi::new(client_b.clone()).sync_order(&order_a.id),
        404,
        "order_not_found",
    );

    let prepared = client_a
        .authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "owned.png",
                "mime_type": "image/png",
                "size_bytes": 68,
                "sha256": VALID_UPLOAD_SHA256,
            })),
            None,
        )
        .expect("prepare user A reference")
        .data;
    let file_a = prepared["file"]["id"]
        .as_str()
        .expect("user A file id")
        .to_string();
    client_a
        .authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/uploads/references/{file_a}/complete"),
            None,
            None,
        )
        .expect("complete user A reference");
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/uploads/references/{file_a}/complete"),
            None,
            None,
        ),
        404,
        "reference_file_not_found",
    );
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_a}"),
            None,
            None,
        ),
        404,
        "reference_file_not_found",
    );
    let foreign_reference = CreateGenerationTask {
        client_request_id: format!("foreign_ref_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: "foreign reference".to_string(),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(vec![file_a.clone()]),
        target_language: None,
    };
    assert_http_error(
        generation_b.create_task(&foreign_reference),
        409,
        "reference_file_unavailable",
    );
    generation_a
        .cancel(&task_a.id)
        .expect("cancel user A fixture");
    generation_b
        .cancel(&task_b.id)
        .expect("cancel user B fixture");
    client_a
        .authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_a}"),
            None,
            None,
        )
        .expect("delete user A fixture");
    AuthApi::new(client_a).logout(false).expect("logout user A");
    AuthApi::new(client_b).logout(false).expect("logout user B");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_payment_required_fields_exact_boundaries_and_repeated_sync() {
    let (client, _) = login_new_user();
    for (path, valid, missing_fields) in [
        (
            "/v1/credits/orders",
            json!({ "pack_code": "pack_1000", "client_request_id": "12345678" }),
            vec!["pack_code", "client_request_id"],
        ),
        (
            "/v1/membership/orders",
            json!({ "plan_code": "basic", "client_request_id": "12345678" }),
            vec!["plan_code", "client_request_id"],
        ),
    ] {
        for field in missing_fields {
            let mut body = valid.clone();
            body.as_object_mut()
                .expect("order body object")
                .remove(field);
            assert_http_error_field(
                client.authenticated_json::<Value>(Method::POST, path, Some(body), None),
                400,
                "validation_failed",
                Some(field),
            );
        }
    }
    assert_http_error_field(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/membership/upgrade-quotes",
            Some(json!({})),
            None,
        ),
        400,
        "validation_failed",
        Some("target_plan_code"),
    );
    for field in ["quote_id", "client_request_id"] {
        let mut body = json!({
            "quote_id": Uuid::new_v4(),
            "client_request_id": "12345678",
        });
        body.as_object_mut()
            .expect("upgrade body object")
            .remove(field);
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/membership/upgrade-orders",
                Some(body),
                None,
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    for (path, field, body) in [
        (
            "/v1/credits/orders",
            "pack_code",
            json!({ "pack_code": "a", "client_request_id": "invalid01" }),
        ),
        (
            "/v1/credits/orders",
            "pack_code",
            json!({ "pack_code": format!("p{}", "x".repeat(32)), "client_request_id": "invalid02" }),
        ),
        (
            "/v1/membership/orders",
            "plan_code",
            json!({ "plan_code": "a", "client_request_id": "invalid03" }),
        ),
        (
            "/v1/membership/orders",
            "plan_code",
            json!({ "plan_code": format!("p{}", "x".repeat(32)), "client_request_id": "invalid04" }),
        ),
    ] {
        assert_http_error_field(
            client.authenticated_json::<Value>(Method::POST, path, Some(body), None),
            400,
            "validation_failed",
            Some(field),
        );
    }

    let payment = PaymentApi::new(client.clone());
    let short_boundary = payment
        .create_credit_order("pack_1000", "12345678")
        .expect("eight character order request id");
    let long_boundary = MembershipApi::new(client.clone())
        .create_order("basic", &"m".repeat(64))
        .expect("64 character order request id");
    for order_id in [&short_boundary.id, &long_boundary.id] {
        let first = payment.sync_order(order_id).expect("first pending sync");
        let second = payment.sync_order(order_id).expect("second pending sync");
        assert_eq!(first.status, "pending_payment");
        assert_eq!(second.status, "pending_payment");
        assert_eq!(first.id, second.id);
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout payment required field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_session_self_revoke_and_logout_all_are_terminal() {
    let (client, login) = login_new_user();
    let current_session = AccountApi::new(client.clone())
        .snapshot()
        .expect("load current session")
        .sessions
        .into_iter()
        .find(|session| session.is_current)
        .expect("current session");
    AccountApi::new(client.clone())
        .revoke_session(&current_session.id)
        .expect("revoke current session");
    assert_http_error(
        client.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
    assert!(client.session().access_token().is_none());

    let (logout_all_client, logout_all_login) = login_new_user();
    let copied_session = new_client_with(
        logout_all_client.device().id.clone(),
        logout_all_client.app_version(),
    );
    copied_session
        .session()
        .install_tokens_for_user(&logout_all_login.tokens, &logout_all_login.user.id)
        .expect("copy session before logout all");
    AuthApi::new(logout_all_client.clone())
        .logout(true)
        .expect("logout all sessions");
    assert!(logout_all_client.session().access_token().is_none());
    assert_http_error(
        copied_session.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
    assert!(copied_session.session().access_token().is_none());
    assert!(!login.tokens.access_token.is_empty());
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_http_route_errors_and_request_ids() {
    let http = reqwest::blocking::Client::new();
    let supplied_request_id = "protocol.request-0001";
    let response = http
        .get(
            base_url()
                .join("/v1/route-does-not-exist")
                .expect("unknown route URL"),
        )
        .header("X-Request-ID", supplied_request_id)
        .send()
        .expect("unknown route response");
    let body = assert_raw_problem(response, 404, "route_not_found");
    assert_eq!(body["request_id"], supplied_request_id);

    let response = http
        .get(
            base_url()
                .join("/v1/another-missing-route")
                .expect("missing route URL"),
        )
        .header("X-Request-ID", "short")
        .send()
        .expect("invalid request ID response");
    let body = assert_raw_problem(response, 404, "route_not_found");
    assert_ne!(body["request_id"], "short");
    assert!(
        body["request_id"]
            .as_str()
            .expect("generated request ID")
            .len()
            >= 8
    );
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_method_not_allowed_uses_json_error_envelope() {
    let http = reqwest::blocking::Client::new();
    let response = http
        .post(base_url().join("/v1").expect("method route URL"))
        .header("X-Request-ID", "method-test-0001")
        .send()
        .expect("method not allowed response");
    assert_eq!(
        response
            .headers()
            .get("Allow")
            .and_then(|value| value.to_str().ok()),
        Some("HEAD, GET")
    );
    assert_eq!(
        response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json; charset=utf-8"),
        "405 responses must use the same JSON envelope as other API errors"
    );
    assert_raw_problem(response, 405, "request_error");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_body_parser_errors_use_json_envelopes() {
    let http = reqwest::blocking::Client::new();
    let response = http
        .post(
            base_url()
                .join("/v1/auth/email/code")
                .expect("bad JSON URL"),
        )
        .header("Content-Type", "application/json")
        .body("{")
        .send()
        .expect("malformed JSON response");
    assert_raw_problem(response, 400, "request_error");

    let oversized = json!({
        "email": format!("{}@example.com", "x".repeat(70_000)),
        "app_version": env!("CARGO_PKG_VERSION"),
    })
    .to_string();
    let response = http
        .post(
            base_url()
                .join("/v1/auth/email/code")
                .expect("oversized JSON URL"),
        )
        .header("Content-Type", "application/json")
        .body(oversized)
        .send()
        .expect("oversized JSON response");
    assert_raw_problem(response, 413, "request_error");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_authentication_header_matrix() {
    let (client, login) = login_new_user();
    let http = reqwest::blocking::Client::new();
    let account_url = base_url().join("/v1/account").expect("account URL");

    let response = http
        .get(account_url.clone())
        .header(
            "Authorization",
            format!("Bearer {}", login.tokens.access_token),
        )
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("Bearer-only response");
    assert_raw_problem(response, 401, "authentication_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", &login.tokens.access_token)
        .send()
        .expect("missing identity headers response");
    assert_raw_problem(response, 400, "client_identity_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .send()
        .expect("missing device header response");
    assert_raw_problem(response, 400, "client_identity_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", &login.tokens.access_token)
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("missing version header response");
    assert_raw_problem(response, 400, "client_identity_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", "not-a-jwt")
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("invalid token response");
    assert_raw_problem(response, 401, "access_token_invalid");

    let response = http
        .get(account_url)
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", "1.0")
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("invalid authenticated client version response");
    assert_raw_problem(response, 400, "client_version_invalid");
    AuthApi::new(client)
        .logout(false)
        .expect("logout header matrix user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_exact_http_success_statuses_and_envelopes() {
    let (client, login) = login_new_user();
    let http = reqwest::blocking::Client::new();
    let request_id = format!("status_{}", Uuid::new_v4().simple());
    let task_body = serde_json::to_value(prompt_request(request_id.clone(), "status contract"))
        .expect("serialize status task");
    let response = http
        .post(
            base_url()
                .join("/v1/generation/tasks")
                .expect("create task URL"),
        )
        .header("Content-Type", "application/json")
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .header("Idempotency-Key", &request_id)
        .header("X-Request-ID", "success-status-0001")
        .json(&task_body)
        .send()
        .expect("raw task creation response");
    assert_eq!(response.status().as_u16(), 202);
    assert_eq!(
        response
            .headers()
            .get("X-Request-ID")
            .and_then(|value| value.to_str().ok()),
        Some("success-status-0001")
    );
    let body: Value = response.json().expect("task success envelope");
    assert_eq!(body["request_id"], "success-status-0001");
    assert!(body["error"].is_null());
    assert!(body.get("meta").is_none());
    let task_id = body["data"]["id"].as_str().expect("created task ID");

    let response = http
        .post(
            base_url()
                .join(&format!("/v1/generation/tasks/{task_id}/cancel"))
                .expect("cancel task URL"),
        )
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("raw task cancellation response");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().expect("cancel success envelope");
    assert!(body["request_id"].is_string());
    assert!(body["error"].is_null());
    assert_eq!(body["data"]["status"], "cancelled");

    let order_request_id = format!("status_order_{}", Uuid::new_v4().simple());
    let response = http
        .post(
            base_url()
                .join("/v1/credits/orders")
                .expect("credit order URL"),
        )
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .header("Idempotency-Key", &order_request_id)
        .json(&json!({
            "pack_code": "pack_1000",
            "client_request_id": order_request_id,
        }))
        .send()
        .expect("raw credit order response");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().expect("order success envelope");
    assert!(body["error"].is_null());
    assert_eq!(body["data"]["status"], "pending_payment");
    AuthApi::new(client)
        .logout(false)
        .expect("logout status contract user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_concurrent_refresh_is_single_flight_against_backend() {
    let (client, _) = login_new_user();
    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let thread_client = client.clone();
            let thread_barrier = barrier.clone();
            std::thread::spawn(move || {
                thread_barrier.wait();
                AuthApi::new(thread_client).refresh()
            })
        })
        .collect();
    let tokens: Vec<String> = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .expect("refresh thread")
                .expect("concurrent refresh")
        })
        .collect();
    assert_eq!(tokens.len(), workers);
    assert!(tokens.iter().all(|token| token == &tokens[0]));
    assert!(!client
        .authenticated_json::<Value>(Method::GET, "/v1/account", None, None)
        .expect("account works after concurrent refresh")
        .request_id
        .is_empty());
    AuthApi::new(client)
        .logout(false)
        .expect("logout concurrent refresh user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_concurrent_generation_idempotency_never_duplicates_resource() {
    let (client, _) = login_new_user();
    let workers = 8;
    let request = Arc::new(prompt_request(
        format!("concurrent_{}", Uuid::new_v4().simple()),
        "same concurrent prompt",
    ));
    let barrier = Arc::new(Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let thread_client = client.clone();
            let thread_request = request.clone();
            let thread_barrier = barrier.clone();
            std::thread::spawn(move || {
                thread_barrier.wait();
                GenerationApi::new(thread_client).create_task(&thread_request)
            })
        })
        .collect();
    let mut task_ids = HashSet::new();
    let mut in_progress = 0;
    for result in handles
        .into_iter()
        .map(|handle| handle.join().expect("generation thread"))
    {
        match result {
            Ok(task) => {
                task_ids.insert(task.id);
            }
            Err(ApiError::Http { status, code, .. }) => {
                assert_eq!(status, 409);
                assert_eq!(code, "request_in_progress");
                in_progress += 1;
            }
            Err(error) => panic!("unexpected concurrent idempotency error: {error:?}"),
        }
    }
    assert_eq!(
        task_ids.len(),
        1,
        "concurrent replays created more than one task"
    );
    assert!(task_ids.len() + in_progress >= 1);
    let task_id = task_ids.into_iter().next().expect("one idempotent task");
    GenerationApi::new(client.clone())
        .cancel(&task_id)
        .expect("cancel concurrent idempotency task");
    AuthApi::new(client)
        .logout(false)
        .expect("logout concurrent idempotency user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_and_ledger_cursor_continuity() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let mut created_ids = Vec::new();
    for index in 0..3 {
        created_ids.push(
            generation
                .create_task(&prompt_request(
                    format!("cursor_{index}_{}", Uuid::new_v4().simple()),
                    &format!("cursor prompt {index}"),
                ))
                .expect("create cursor task")
                .id,
        );
    }
    let first = client
        .authenticated_json::<Value>(Method::GET, "/v1/generation/tasks?limit=2", None, None)
        .expect("first task page")
        .data;
    let first_items = first["items"].as_array().expect("first task items");
    assert_eq!(first_items.len(), 2);
    let cursor = first["next_cursor"].as_str().expect("task next cursor");
    let second = client
        .authenticated_json::<Value>(
            Method::GET,
            &format!("/v1/generation/tasks?limit=2&cursor={cursor}"),
            None,
            None,
        )
        .expect("second task page")
        .data;
    let second_items = second["items"].as_array().expect("second task items");
    assert_eq!(second_items.len(), 1);
    assert!(second["next_cursor"].is_null());
    let first_ids: HashSet<_> = first_items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(second_items
        .iter()
        .all(|item| !first_ids.contains(item["id"].as_str().expect("second task ID"))));

    let ledger_first = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            "/v1/credits/ledger?limit=1",
            None,
            None,
        )
        .expect("first ledger page");
    assert_eq!(ledger_first.data.len(), 1);
    let ledger_cursor = ledger_first
        .meta
        .as_ref()
        .and_then(|meta| meta.next_cursor.as_deref())
        .expect("ledger next cursor");
    let ledger_second = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            &format!("/v1/credits/ledger?limit=1&cursor={ledger_cursor}"),
            None,
            None,
        )
        .expect("second ledger page");
    assert_eq!(ledger_second.data.len(), 1);
    assert_ne!(ledger_first.data[0].id, ledger_second.data[0].id);
    for task_id in created_ids {
        generation.cancel(&task_id).expect("cancel cursor fixture");
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout cursor continuity user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_reference_attachment_lifecycle_prevents_delete_and_reuse() {
    let (client, _) = login_new_user();
    let path = std::env::temp_dir().join(format!(
        "artforge-reference-lifecycle-{}.png",
        Uuid::new_v4()
    ));
    std::fs::write(&path, MOCK_PNG).expect("write lifecycle reference fixture");
    let generation = GenerationApi::new(client.clone());
    let file_id = generation
        .upload_reference(&path)
        .expect("upload lifecycle reference");
    let _ = std::fs::remove_file(&path);
    let request = CreateGenerationTask {
        client_request_id: format!("reference_owner_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: "reference owner".to_string(),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(vec![file_id.clone()]),
        target_language: None,
    };
    let task = generation
        .create_task(&request)
        .expect("attach lifecycle reference");
    assert_eq!(task.request["reference_file_ids"], json!([file_id]));
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_id}"),
            None,
            None,
        ),
        409,
        "reference_file_in_use",
    );
    let replay_reference = CreateGenerationTask {
        client_request_id: format!("reference_reuse_{}", Uuid::new_v4().simple()),
        prompt: "reference reuse".to_string(),
        ..request
    };
    assert_http_error(
        generation.create_task(&replay_reference),
        409,
        "reference_file_unavailable",
    );
    generation
        .cancel(&task.id)
        .expect("cancel reference lifecycle task");
    AuthApi::new(client)
        .logout(false)
        .expect("logout reference lifecycle user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_catalog_and_account_dto_invariants() {
    let (client, login) = login_new_user();
    let snapshot = AccountApi::new(client.clone())
        .snapshot()
        .expect("load invariant snapshot");
    assert!(Uuid::parse_str(&snapshot.account.user.id).is_ok());
    assert_eq!(snapshot.account.user.id, login.user.id);
    assert_eq!(snapshot.account.user.status, "active");
    assert!(snapshot.account.user.registered_at.contains('T'));
    let membership = snapshot
        .account
        .membership
        .as_ref()
        .expect("owner account snapshot includes membership");
    assert!(membership.revision.parse::<u64>().is_ok());
    if let Some(plan) = membership.plan.as_ref() {
        assert!(!plan.max_quality.is_empty());
    }
    let credits = snapshot.account.credits.as_ref().expect("credit account");
    for value in [
        &credits.available,
        &credits.reserved,
        &credits.lifetime_granted,
        &credits.lifetime_spent,
    ] {
        assert!(
            value.parse::<u64>().is_ok(),
            "credit amount is not an unsigned decimal: {value}"
        );
    }

    let plans = snapshot.plans.as_deref().expect("owner plans");
    let packs = snapshot.packs.as_deref().expect("owner credit packs");
    let models = snapshot.models.as_deref().expect("owner model catalog");
    let ledger = snapshot.ledger.as_deref().expect("owner credit ledger");
    let plan_codes: HashSet<_> = plans.iter().map(|plan| plan.code.as_str()).collect();
    assert_eq!(plan_codes.len(), plans.len());
    for plan in plans {
        assert!(!plan.name.is_empty());
        assert!(plan.version > 0);
        assert!(plan.tier_rank >= 0);
        assert!(plan.price_cents.parse::<u64>().is_ok());
        assert!(plan.period_days > 0);
        assert!(plan.grant_credits.parse::<u64>().is_ok());
        assert!((0..=10_000).contains(&plan.recharge_discount_bps));
        assert!(plan.entitlements.is_object());
    }
    let pack_codes: HashSet<_> = packs.iter().map(|pack| pack.code.as_str()).collect();
    assert_eq!(pack_codes.len(), packs.len());
    for pack in packs {
        assert!(!pack.name.is_empty());
        assert!(pack.price_cents.parse::<u64>().expect("pack price") > 0);
        assert!(pack.credits.parse::<u64>().expect("pack credits") > 0);
    }
    let model_codes: HashSet<_> = models.iter().map(|model| model.code.as_str()).collect();
    assert_eq!(model_codes.len(), models.len());
    for model in models {
        assert!(model.version > 0);
        assert!(!model.name.is_empty());
        assert!(["image_generation", "prompt_processing"].contains(&model.purpose.as_str()));
        assert!(model.capabilities.is_object());
        assert!(!model.prices.is_empty());
        for price in &model.prices {
            assert!(["standard", "1K", "2K", "4K"].contains(&price.quality.as_str()));
            assert!(price.credit_cost.parse::<u64>().expect("model credit cost") > 0);
            if let Some(edge) = price.max_long_edge {
                assert!(edge > 0);
            }
        }
    }
    assert_eq!(
        snapshot
            .sessions
            .iter()
            .filter(|session| session.is_current)
            .count(),
        1
    );
    for session in &snapshot.sessions {
        assert!(Uuid::parse_str(&session.id).is_ok());
        assert!(["windows", "macos"].contains(&session.platform.as_str()));
        assert_eq!(session.app_version.split('.').count(), 3);
        assert!(session.last_seen_at.contains('T'));
    }
    for entry in ledger {
        assert!(!entry.entry_type.is_empty());
        assert!(entry.available_delta.parse::<i128>().is_ok());
        assert!(entry.reserved_delta.parse::<i128>().is_ok());
        assert!(entry.available_after.parse::<u128>().is_ok());
        assert!(entry.reserved_after.parse::<u128>().is_ok());
        assert!(!entry.business_type.is_empty());
        assert!(!entry.description.is_empty());
        assert!(entry.created_at.contains('T'));
    }
    AuthApi::new(client)
        .logout(false)
        .expect("logout DTO invariant user");
}

#[test]
fn cross_stack_strict_account_finance_fixture_matches_server_projection() {
    let membership: AccountMembership = serde_json::from_value(json!({
        "revision": "3",
        "period_id": "2026-09",
        "starts_at": "2026-09-01T00:00:00Z",
        "ends_at": "2026-10-01T00:00:00Z",
        "plan": {
            "code": "pro",
            "name": "Pro",
            "tier_rank": 2,
            "recharge_discount_bps": 9000,
            "max_quality": "4K"
        }
    }))
    .expect("strict membership finance projection");
    let credits: CreditAccount = serde_json::from_value(json!({
        "available": "500",
        "reserved": "12",
        "lifetime_granted": "900",
        "lifetime_spent": "400",
        "version": "8"
    }))
    .expect("strict credit finance projection");

    assert_eq!(
        membership.plan.expect("non-null fixture plan").max_quality,
        "4K"
    );
    assert_eq!(credits.version, "8");
}

fn setup_cross_stack_password(
    client: &ApiClient,
    login: &LoginResponse,
    new_password: &str,
) -> SessionScope {
    let scope = client
        .session()
        .scope_for_user(&login.user.id)
        .expect("registered user has a current session scope");
    let account = AccountApi::new(client.clone());
    let delivery = account
        .request_password_code_scoped(&scope)
        .expect("request password verification code");
    assert!(delivery.expires_in_seconds > 0);
    assert!(delivery.resend_after_seconds > 0);
    assert!(!delivery.email_masked.is_empty());
    let mutation = account
        .set_password_scoped(new_password, None, Some(MOCK_PASSWORD_CODE), &scope)
        .expect("set password with verified email code");
    assert!(mutation.set);
    assert!(!mutation.changed_at.is_empty());
    assert!(!mutation.other_sessions_revoked);
    scope
}

fn install_password_login(client: &ApiClient, response: &LoginResponse) -> SessionScope {
    client
        .session()
        .install_tokens_for_user(&response.tokens, &response.user.id)
        .expect("install password login session")
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_password_setup_logout_and_login() {
    let (client, login, email) = login_new_user_with_email("password-setup");
    setup_cross_stack_password(&client, &login, PASSWORD_ALPHA);
    let snapshot = AccountApi::new(client.clone())
        .snapshot()
        .expect("load account after password setup");
    assert!(snapshot.account.auth_methods.password.set);
    AuthApi::new(client)
        .logout(false)
        .expect("logout email-code session");

    let password_client = new_client();
    let password_auth = AuthApi::new(password_client.clone());
    let password_login = password_auth
        .password_login_response(
            &email,
            PASSWORD_ALPHA,
            &agreement_acceptances(&password_auth),
        )
        .expect("login with configured password");
    assert!(!password_login.is_new_user);
    install_password_login(&password_client, &password_login);
    let snapshot = AccountApi::new(password_client.clone())
        .snapshot()
        .expect("load account after password login");
    assert_eq!(snapshot.account.user.id, login.user.id);
    assert!(snapshot.account.auth_methods.password.set);
    password_auth
        .logout(false)
        .expect("logout password session");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_password_change_keeps_device_a_and_revokes_device_b() {
    let (client_a, login_a, email) = login_new_user_with_email("password-change");
    let scope_a = setup_cross_stack_password(&client_a, &login_a, PASSWORD_ALPHA);

    let client_b = new_client();
    let auth_b = AuthApi::new(client_b.clone());
    let login_b = auth_b
        .password_login_response(&email, PASSWORD_ALPHA, &agreement_acceptances(&auth_b))
        .expect("login second device before password change");
    install_password_login(&client_b, &login_b);

    let mutation = AccountApi::new(client_a.clone())
        .set_password_scoped(PASSWORD_BETA, Some(PASSWORD_ALPHA), None, &scope_a)
        .expect("change password with current password");
    assert!(mutation.set);
    assert!(mutation.other_sessions_revoked);
    assert!(!mutation.changed_at.is_empty());
    let snapshot_a = AccountApi::new(client_a.clone())
        .snapshot()
        .expect("current device remains authenticated after password change");
    assert_eq!(snapshot_a.account.user.id, login_a.user.id);

    assert_http_error(
        client_b.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
    assert!(client_b.session().access_token().is_none());
    AuthApi::new(client_a)
        .logout(false)
        .expect("logout surviving password-change session");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_password_reset_revokes_old_sessions_and_installs_device_c() {
    let (client_a, login_a, email) = login_new_user_with_email("password-reset");
    setup_cross_stack_password(&client_a, &login_a, PASSWORD_ALPHA);

    let client_b = new_client();
    let auth_b = AuthApi::new(client_b.clone());
    let login_b = auth_b
        .password_login_response(&email, PASSWORD_ALPHA, &agreement_acceptances(&auth_b))
        .expect("login second device before password reset");
    install_password_login(&client_b, &login_b);

    let client_c = new_client();
    let auth_c = AuthApi::new(client_c.clone());
    let reset_delivery = auth_c
        .request_password_reset_code(&email)
        .expect("request signed-out password reset code");
    assert!(reset_delivery.accepted);
    assert!(!reset_delivery.message.is_empty());
    let reset_login = auth_c
        .reset_password_response(
            &email,
            MOCK_PASSWORD_CODE,
            PASSWORD_RESET,
            &agreement_acceptances(&auth_c),
        )
        .expect("reset password and receive a fresh session");
    assert_eq!(reset_login.user.id, login_a.user.id);
    install_password_login(&client_c, &reset_login);

    for revoked_client in [&client_a, &client_b] {
        assert_http_error(
            revoked_client.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
            401,
            "session_invalid",
        );
        assert!(revoked_client.session().access_token().is_none());
    }
    let snapshot_c = AccountApi::new(client_c.clone())
        .snapshot()
        .expect("reset session authenticates on device C");
    assert_eq!(snapshot_c.account.user.id, login_a.user.id);
    assert!(snapshot_c.account.auth_methods.password.set);
    auth_c.logout(false).expect("logout password-reset session");
}

#[test]
#[ignore = "kept with the password cross-stack acceptance group"]
fn cross_stack_password_legacy_snapshot_defaults_to_unset() {
    let snapshot: AccountSnapshot = serde_json::from_value(json!({
        "user": {
            "id": Uuid::new_v4(),
            "email_masked": "l***y@example.com",
            "nickname": null,
            "status": "active",
            "registered_at": "2026-08-19T00:00:00.000Z"
        },
        "auth_methods": {
            "email": { "bound": true },
            "wechat": { "bound": false, "can_unbind": false, "nickname": null }
        },
        "membership": {
            "revision": "0",
            "period_id": null,
            "starts_at": null,
            "ends_at": null,
            "plan": null
        },
        "credits": null
    }))
    .expect("deserialize account snapshot from a pre-password server");

    assert!(!snapshot.auth_methods.password.set);
}

#[test]
#[ignore = "requires the dev Mock API server with minimum client version above 0.0.0"]
fn cross_stack_password_preserves_agreement_and_client_version_errors() {
    let (client, login, email) = login_new_user_with_email("password-errors");
    setup_cross_stack_password(&client, &login, PASSWORD_ALPHA);
    let acceptances = agreement_acceptances(&AuthApi::new(client.clone()));

    let password_client = new_client();
    let password_auth = AuthApi::new(password_client);
    assert_http_error(
        password_auth.password_login_response(&email, PASSWORD_ALPHA, &[]),
        428,
        "agreement_acceptance_required",
    );

    let reset_client = new_client();
    let reset_auth = AuthApi::new(reset_client);
    reset_auth
        .request_password_reset_code(&email)
        .expect("request reset code for error-contract checks");
    assert_http_error(
        reset_auth.reset_password_response(&email, MOCK_PASSWORD_CODE, PASSWORD_RESET, &[]),
        428,
        "agreement_acceptance_required",
    );

    let outdated_login_client = new_client_with(
        format!("password-outdated-login-{}", Uuid::new_v4()),
        OUTDATED_PASSWORD_CLIENT_VERSION,
    );
    assert_http_error(
        AuthApi::new(outdated_login_client).password_login_response(
            &email,
            PASSWORD_ALPHA,
            &acceptances,
        ),
        426,
        "client_upgrade_required",
    );
    let outdated_reset_client = new_client_with(
        format!("password-outdated-reset-{}", Uuid::new_v4()),
        OUTDATED_PASSWORD_CLIENT_VERSION,
    );
    assert_http_error(
        AuthApi::new(outdated_reset_client).reset_password_response(
            &email,
            MOCK_PASSWORD_CODE,
            PASSWORD_RESET,
            &acceptances,
        ),
        426,
        "client_upgrade_required",
    );

    AuthApi::new(client)
        .logout(false)
        .expect("logout password error-contract session");
}
