//! Account-center-only team operations. Workers own a bounded real-user unit.
use super::*;
#[path = "team_accounts/presentation.rs"]
mod presentation;
#[path = "team_accounts/forms.rs"]
mod forms;
#[cfg(test)]
thread_local!{static TEAM_AFTER_SEND:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};}

pub(super) fn prepare_activation_team_projection(groups:&[AccountGroupChoice],current:&BillingScope,snapshot:&AccountSnapshot) -> PreparedUiProjection {
    let mut ui=PreparedUiProjection::default();
    ui.push((), |state, _| presentation::clear_forms_and_feedback(state));
    ui.push((), |state, _| presentation::clear_pages(state));
    presentation::prepare_summary(&mut ui, groups, Some(snapshot));
    let choice = Some(snapshot).map(|snapshot| &snapshot.billing_group);
    let cap = |capability| choice.is_some_and(|choice| choice.has_capability(capability));
    ui.push(Some(current).map(|scope| scope.request.account_group_id.clone()).unwrap_or_default().into(), |state, value| state.set_selected_account_group_id(value));
    ui.push(Some(snapshot).is_some_and(|snapshot| snapshot.read_only), |state, value| state.set_account_group_read_only(value));
    ui.push(choice.map(|choice| team_amount(choice, Some(snapshot))).unwrap_or_default().into(), |state, value| state.set_account_amount_label(value));
    ui.push(cap(KnownCapability::ManageGroup), |state, value| state.set_team_can_manage_group(value));
    ui.push(cap(KnownCapability::ManageMembers), |state, value| state.set_team_can_manage_members(value));
    ui.push(cap(KnownCapability::ManageInvitations), |state, value| state.set_team_can_manage_invitations(value));
    ui.push(cap(KnownCapability::ReadGroupUsage), |state, value| state.set_team_can_read_usage(value));
    ui.push(cap(KnownCapability::ReadGroupFinance), |state, value| state.set_team_can_read_finance(value));
    ui.push(cap(KnownCapability::LeaveTeam), |state, value| state.set_team_can_leave(value));
    ui.push(cap(KnownCapability::Purchase), |state, value| state.set_team_can_purchase(value));
    ui.push(cap(KnownCapability::Redeem), |state, value| state.set_team_can_redeem(value));
    ui.push(choice.map(|choice| choice.name.clone()).unwrap_or_default().into(), |state, value| state.set_team_name_input(value));
    ui.push(ModelRc::new(VecModel::from(groups.iter()
        .map(|choice| team_card(choice, Some(current), Some(snapshot))).collect::<Vec<_>>())), |state, value| state.set_account_groups(value));
    ui
}

#[cfg(test)]
mod core_team_ui_patch_tests {
    use super::*;
    use std::io::{Read,Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;
    use backend_generation::billing_capture_test_support::{listener, read_request};

    struct JoinedFixtureTransport(Option<std::thread::JoinHandle<()>>);
    impl JoinedFixtureTransport {
        fn spawn(work: impl FnOnce() + Send + 'static) -> Self { Self(Some(std::thread::spawn(work))) }
        fn join(mut self) { self.0.take().unwrap().join().expect("fixture transport panicked"); }
    }
    impl Drop for JoinedFixtureTransport {
        fn drop(&mut self) { if let Some(worker) = self.0.take() { let _ = worker.join(); } }
    }

    const USER: &str = "11111111-1111-4111-8111-111111111111";
    const GROUP: &str = "22222222-2222-4222-8222-222222222222";
    const MEMBER: &str = "33333333-3333-4333-8333-333333333333";
    // Break caught: member billing snapshots carry the authoritative quota at the
    // snapshot boundary even when the lightweight group choice has no quota.
    #[test]
    fn core_team_ui_member_activation_uses_snapshot_quota_without_finance() {
        let (_fixture, app, context) = setup("http://127.0.0.1:9");
        let mut snapshot = context.billing_context.confirmed_snapshot().unwrap();
        snapshot.billing_group.role = "member".into();
        snapshot.billing_group.capabilities = vec!["read_own_quota".into(), "leave_team".into()];
        snapshot.billing_group.quota = None;
        snapshot.quota = Some(serde_json::from_value(serde_json::json!({
            "period_start":"2026-09-01T00:00:00Z", "period_end":"2026-10-01T00:00:00Z",
            "monthly_limit":"500", "settled":"80", "reserved":"20", "remaining":"400"
        })).unwrap());
        let scope = context.billing_context.confirmed_scope().unwrap();
        prepare_activation_team_projection(&[snapshot.billing_group.clone()], &scope, &snapshot)
            .publish(&app.global::<AppState>());
        let state = app.global::<AppState>();
        assert!(!state.get_team_can_read_finance());
        assert!(!state.get_team_can_manage_members());
        assert!(state.get_account_amount_label().contains("400"), "member quota missing: {}", state.get_account_amount_label());
        assert_eq!(state.get_team_quota_monthly(), "500");
        assert_eq!(state.get_team_quota_settled(), "80");
        assert_eq!(state.get_team_quota_reserved(), "20");
        assert_eq!(state.get_team_quota_remaining(), "400");
    }

    // Break caught: a retired account must not supply a previous form's credit
    // amount to a later team's member or invitation mutation.
    #[test]
    fn core_team_ui_retirement_clears_numeric_form_and_feedback() {
        let (_fixture, app, context) = setup("http://127.0.0.1:9");
        let state = app.global::<AppState>();
        state.set_team_limit_input("500".into());
        state.set_team_page_error("previous team error".into());
        clear_team_context(&app, &context);
        assert_eq!(state.get_team_limit_input(), "0");
        assert_eq!(state.get_team_page_error(), "");
        assert_eq!(state.get_team_form_id(), "");
        assert_eq!(state.get_team_current_name(), "");
    }

    #[test]
    fn core_team_ui_toolbar_actions_and_both_model_selectors_fit_supported_windows() {
        use i_slint_backend_testing::ElementHandle;
        let (_fixture, app, _) = setup("http://127.0.0.1:9");
        let state = app.global::<AppState>();
        state.set_profile_open(false);
        state.set_page("generation".into());
        state.set_nickname("测试昵称".into());
        app.show().unwrap();
        for width in [1180.0, 1364.0, 1600.0] {
            for logged_in in [true, false] {
                for payment in [false, true] {
                    state.set_logged_in(logged_in);
                    state.set_payment_active(payment);
                    app.window().set_size(slint::LogicalSize::new(width, 928.0));
                    let bar = ElementHandle::find_by_element_type_name(&app, "TopBar").next().unwrap();
                    let left = bar.absolute_position().x;
                    let right = left + bar.size().width;
                    let actions = bar.query_descendants().match_inherits("PillButton").find_all();
                    for action in actions {
                        assert!(action.absolute_position().x >= left && action.absolute_position().x + action.size().width <= right + 1.0,
                            "toolbar action overflows at width {width}, logged in {logged_in}, payment {payment}");
                    }
                    let pickers = bar.query_descendants().match_inherits("ModelPicker").find_all();
                    assert_eq!(pickers.len(), 2);
                    for picker in pickers {
                        assert!(picker.size().width >= 220.0, "model selection must remain usable");
                        assert!(picker.absolute_position().x >= left && picker.absolute_position().x + picker.size().width <= right + 1.0);
                    }
                }
            }
        }
    }

    #[test]
    fn core_team_ui_member_limit_edit_prefills_row_and_cancel_never_mutates() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let (_fixture, app, context) = setup(&format!("http://{}/", listener.local_addr().unwrap()));
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        state.set_team_limit_input("999".into());
        state.set_team_members(ModelRc::new(VecModel::from(vec![TeamMemberRowView {
            member_id: MEMBER.into(), display_name: "小雨".into(), email_masked: "xi***@example.com".into(),
            status: "active".into(), monthly_limit: "500".into(), settled: "80".into(),
            reserved: "20".into(), remaining: "400".into(), version: "7".into(), ..Default::default()
        }])));
        app.window().set_size(slint::LogicalSize::new(1364.0, 928.0));
        app.show().unwrap();
        let header = ElementHandle::find_by_element_id(&app, "TeamMembersPanel::member-header").next().unwrap();
        let row = ElementHandle::find_by_element_id(&app, "TeamMembersPanel::member-row").next().unwrap();
        assert!(header.absolute_position().y + header.size().height <= row.absolute_position().y + 1.0,
            "table headings must remain above the first member row");
        ElementHandle::find_by_accessible_label(&app, "设置额度").next().expect("quota edit action")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(state.get_team_limit_input(), "500", "row edit must prefill its own quota");
        assert_eq!(state.get_team_form_id(), MEMBER);
        assert_eq!(state.get_team_form_version(), "7");
        ElementHandle::find_by_accessible_label(&app, "取消").next().expect("cancelable member form")
            .mock_single_click(PointerEventButton::Left);
        pump_for(Duration::from_millis(100));
        assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "opening or canceling a quota form must never start a mutation");
    }

    fn form_member() -> TeamMemberRowView {
        TeamMemberRowView { member_id: MEMBER.into(), display_name: "小雨".into(), status: "active".into(),
            monthly_limit: "500".into(), settled: "80".into(), reserved: "20".into(), remaining: "400".into(),
            version: "7".into(), ..Default::default() }
    }

    #[test]
    fn core_team_ui_refinement_rename_modal_keeps_tabs_stable_and_cancel_never_writes() {
        use i_slint_backend_testing::ElementHandle;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let (_fixture, app, context) = setup(&format!("http://{}/", listener.local_addr().unwrap()));
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        app.show().unwrap();
        let before = ElementHandle::find_by_accessible_label(&app, "成员").next().unwrap().absolute_position();
        ElementHandle::find_by_accessible_label(&app, "修改名称").next().unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert!(ElementHandle::find_by_element_type_name(&app, "TeamFormDialog").next().is_some(),
            "rename must use the existing modal instead of expanding the page");
        let after = ElementHandle::find_by_accessible_label(&app, "成员").next().unwrap().absolute_position();
        assert!((after.y - before.y).abs() < 1.0, "opening rename must not displace navigation");
        assert_eq!(state.get_team_name_input(), "Fixture");
        state.set_team_name_input("Unsaved name".into());
        ElementHandle::find_by_accessible_label(&app, "取消").next().unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert!(state.get_team_form_action().is_empty());
        assert_eq!(state.get_team_current_name(), "Fixture");
        pump_for(Duration::from_millis(50));
        assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn core_team_ui_refinement_received_invitation_is_a_stable_peer_tab_for_both_roles() {
        use i_slint_backend_testing::ElementHandle;
        let (_fixture, app, context) = setup("http://127.0.0.1:9");
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        state.set_team_can_read_usage(true);
        state.set_pending_team_invitation_count("2".into());
        state.on_load_pending_team_invitations(|_| {});
        state.on_load_team_invitations(|_| {});
        app.show().unwrap();
        let members = ElementHandle::find_by_accessible_label(&app, "成员").next().unwrap();
        let received = ElementHandle::find_by_accessible_label(&app, "收到的邀请 2").next().unwrap();
        assert!((received.absolute_position().y - members.absolute_position().y).abs() < 1.0,
            "received invitations must be a permanent peer, not a header action");
        let initial = received.absolute_position();
        received.mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(state.get_team_tab(), "pending");
        assert_eq!(ElementHandle::find_by_accessible_label(&app, "收到的邀请 2").count(), 1);
        assert!((ElementHandle::find_by_accessible_label(&app, "收到的邀请 2").next().unwrap().absolute_position().x - initial.x).abs() < 1.0);
        ElementHandle::find_by_accessible_label(&app, "发出的邀请").next().unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        app.window().dispatch_event(slint::platform::WindowEvent::KeyPressed { text: slint::platform::Key::Tab.into() });
        app.window().dispatch_event(slint::platform::WindowEvent::KeyReleased { text: slint::platform::Key::Tab.into() });
        app.window().dispatch_event(slint::platform::WindowEvent::KeyPressed { text: " ".into() });
        app.window().dispatch_event(slint::platform::WindowEvent::KeyReleased { text: " ".into() });
        assert_eq!(state.get_team_tab(), "pending", "keyboard navigation must still activate the next invitation tab");
        state.set_team_can_manage_members(false);
        state.set_team_can_manage_invitations(false);
        state.set_team_can_read_usage(false);
        state.set_team_has_own_quota(true);
        state.set_team_tab("accounts".into());
        assert!(ElementHandle::find_by_accessible_label(&app, "发出的邀请").next().is_none());
        assert!(ElementHandle::find_by_accessible_label(&app, "用量记录").next().is_none());
        ElementHandle::find_by_accessible_label(&app, "收到的邀请 2").next().unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(state.get_team_tab(), "pending");
        ElementHandle::find_by_accessible_label(&app, "我的额度").next().unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(state.get_team_tab(), "accounts");
    }

    #[test]
    fn core_team_ui_refinement_team_summary_has_no_purchase_or_redeem_actions() {
        use i_slint_backend_testing::ElementHandle;
        let (_fixture, app, context) = setup("http://127.0.0.1:9");
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_can_read_finance(true);
        state.set_team_can_purchase(true);
        state.set_team_can_redeem(true);
        state.set_account_amount_label("170 积分".into());
        app.show().unwrap();
        for label in ["充值与订单", "兑换"] {
            assert!(ElementHandle::find_by_accessible_label(&app, label).next().is_none(), "removed summary action remains: {label}");
        }
        assert!(ElementHandle::find_by_accessible_label(&app, "170 积分").next().is_some(), "credit visibility is retained");
    }

    #[test]
    fn core_team_ui_form_confirm_uses_captured_member_and_version_accepts_zero() {
        let mut transport = ControlledTeamTransport::new(2);
        let (_fixture, app, context) = setup(&transport.url);
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        state.set_team_members(ModelRc::new(VecModel::from(vec![form_member()])));
        state.set_team_invite_limit_input("999".into());
        state.invoke_open_team_form("set_limit".into(), MEMBER.into(), "7".into());
        assert_eq!(state.get_team_limit_input(), "500");
        // Repeated-model changes and presentation IDs cannot retarget the saved confirmation.
        state.set_team_form_id("different-row".into());
        state.set_team_form_version("99".into());
        state.set_team_limit_input("0".into());
        state.invoke_confirm_team_form();
        transport.wait();
        let response = member_page("8", None);
        let envelope: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        transport.reply(0, controlled_response(200, envelope["data"]["items"][0].clone(), None, None));
        let refreshed = Cell::new(false);
        pump_until(|| { if transport.seen.try_recv().is_ok() { refreshed.set(true); } refreshed.get() });
        transport.reply(1, member_page("8", None));
        pump_until(|| !state.get_team_page_loading());
        assert_eq!(state.get_team_form_action(), "");
        assert!(!state.get_team_page_status().is_empty());
        let requests = transport.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with(&format!("PATCH /v1/account-groups/{GROUP}/members/{MEMBER} ")));
        let body: Value = serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body, serde_json::json!({"action":"set_limit", "expected_version":"7", "monthly_credit_limit":"0"}));
    }

    #[test]
    fn core_team_ui_invalid_form_input_is_preserved_and_closed_profile_cannot_submit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let (_fixture, app, context) = setup(&format!("http://{}/", listener.local_addr().unwrap()));
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        state.set_team_members(ModelRc::new(VecModel::from(vec![form_member()])));
        state.invoke_open_team_form("set_limit".into(), MEMBER.into(), "7".into());
        for invalid in ["-1", "不限", ""] {
            state.set_team_limit_input(invalid.into());
            state.invoke_confirm_team_form();
            assert_eq!(state.get_team_limit_input(), invalid);
            assert_eq!(state.get_team_form_action(), "set_limit");
            assert!(!state.get_team_page_error().is_empty());
        }
        state.set_team_limit_input("500".into());
        state.set_profile_open(false);
        state.invoke_confirm_team_form();
        assert!(state.get_team_form_action().is_empty());
        pump_for(Duration::from_millis(100));
        assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn core_team_ui_billing_switch_invalidates_pending_member_form() {
        let (_fixture, app, context) = setup("http://127.0.0.1:9");
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_members(ModelRc::new(VecModel::from(vec![form_member()])));
        state.invoke_open_team_form("remove".into(), MEMBER.into(), "7".into());
        assert_eq!(state.get_team_form_action(), "remove");
        context.billing_context.invalidate_current_billing();
        state.invoke_confirm_team_form();
        assert_eq!(state.get_team_form_action(), "");
        assert!(!state.get_team_page_loading());
    }

    #[test]
    fn core_team_ui_member_account_overview_hides_owner_finance_and_purchase_routes() {
        use i_slint_backend_testing::ElementHandle;
        let (_fixture, app, _) = setup("http://127.0.0.1:9");
        let state = app.global::<AppState>();
        state.set_team_has_own_quota(true);
        state.set_team_can_read_finance(false);
        state.set_team_can_purchase(false);
        state.set_team_current_name("Joined team".into());
        state.set_team_quota_remaining("400".into());
        state.set_credit_balance("777777".into());
        state.set_membership_plan_name("Private owner plan".into());
        state.set_account_center_section("overview".into());
        app.show().unwrap();
        for private in ["777777", "Private owner plan", "前往充值", "查看会员"] {
            assert!(ElementHandle::find_by_accessible_label(&app, private).next().is_none(), "member sees owner finance: {private}");
        }
        assert!(ElementHandle::find_by_accessible_label(&app, "400").next().is_some());
        ElementHandle::find_by_accessible_label(&app, "查看我的额度").next().unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(state.get_account_center_section(), "accounts-teams");
        assert!(state.get_profile_open());
    }

    fn wait_for_request(transport: &ControlledTeamTransport, expected: usize) {
        let arrived = Cell::new(false);
        pump_until(|| {
            if let Ok(index) = transport.seen.try_recv() { assert_eq!(index, expected); arrived.set(true); }
            arrived.get()
        });
    }

    #[test]
    fn core_team_ui_conflict_review_preserves_member_draft_and_retries_latest_version() {
        let mut transport = ControlledTeamTransport::new(4);
        let (_fixture, app, context) = setup(&transport.url);
        render_team_context(&app, &context);
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        state.set_team_members(ModelRc::new(VecModel::from(vec![form_member()])));
        state.invoke_open_team_form("set_limit".into(), MEMBER.into(), "7".into());
        state.set_team_limit_input("650".into());
        state.invoke_confirm_team_form(); transport.wait();
        transport.reply(0, controlled_response(409, Value::Null, None, Some("membership_version_conflict")));
        wait_for_request(&transport, 1);
        transport.reply(1, member_page("8", None));
        pump_until(|| !state.get_team_page_loading());
        assert_eq!(state.get_team_limit_input(), "650");
        assert_eq!(state.get_team_form_action(), "set_limit");
        assert!(state.get_team_form_needs_review(), "conflict must require explicit review before retry");
        state.invoke_confirm_team_form();
        pump_for(Duration::from_millis(60));
        assert!(transport.seen.try_recv().is_err(), "conflict must never automatically replay stale mutation");
        state.invoke_review_team_form();
        assert_eq!(state.get_team_form_version(), "8");
        assert_eq!(state.get_team_limit_input(), "650");
        assert!(!state.get_team_form_needs_review());
        assert!(transport.seen.try_recv().is_err(), "review is not confirmation");
        state.invoke_confirm_team_form(); transport.wait();
        let response = member_page("9", None);
        let body: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        transport.reply(2, controlled_response(200, body["data"]["items"][0].clone(), None, None));
        wait_for_request(&transport, 3); transport.reply(3, member_page("9", None));
        pump_until(|| !state.get_team_page_loading());
        assert!(state.get_team_form_action().is_empty());
        let requests = transport.finish();
        let writes: Vec<Value> = requests.iter().filter(|request| request.starts_with("PATCH "))
            .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()).collect();
        assert_eq!(writes, vec![serde_json::json!({"action":"set_limit","expected_version":"7","monthly_credit_limit":"650"}),
            serde_json::json!({"action":"set_limit","expected_version":"8","monthly_credit_limit":"650"})]);
    }

    fn group_metadata(version: &str, name: &str) -> Value {
        serde_json::json!({"group_id":GROUP,"name":name,"group_status":"active","role":"owner",
            "member_id":null,"relationship_status":null,"readable_context":true,"selectable":true,
            "group_version":version,"membership_version":null,"capabilities":["bill","manage_group","manage_members","manage_invitations"],"quota":null})
    }

    #[test]
    fn core_team_ui_conflict_review_preserves_same_team_rename_draft_and_retries_latest_version() {
        let mut transport = ControlledTeamTransport::new(4);
        let (_fixture, app, context) = setup(&transport.url);
        render_team_context(&app, &context);
        let original_billing = context.billing_context.confirmed_scope();
        let state = app.global::<AppState>();
        state.set_team_tab("members".into());
        state.invoke_open_team_form("rename".into(), "".into(), "".into());
        state.set_team_name_input("My saved draft".into());
        state.invoke_confirm_team_form(); transport.wait();
        transport.reply(0, controlled_response(409, Value::Null, None, Some("account_group_version_conflict")));
        wait_for_request(&transport, 1);
        transport.reply(1, controlled_response(200, serde_json::json!({"items":[group_metadata("2", "Changed elsewhere")],"pending_invitation_count":0}), None, None));
        pump_until(|| !state.get_team_page_loading());
        pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_form_action(), "rename");
        assert_eq!(state.get_team_name_input(), "My saved draft");
        assert_eq!(context.billing_context.confirmed_scope(), original_billing, "metadata-only conflict must not reactivate billing");
        assert!(state.get_team_form_needs_review());
        state.invoke_review_team_form();
        assert_eq!(state.get_team_form_version(), "2");
        assert_eq!(state.get_team_name_input(), "My saved draft");
        assert!(transport.seen.try_recv().is_err(), "review must not submit the rename");
        state.invoke_confirm_team_form(); transport.wait();
        transport.reply(2, controlled_response(200, group_metadata("3", "My saved draft"), None, None));
        wait_for_request(&transport, 3);
        transport.reply(3, controlled_response(200, serde_json::json!({"items":[group_metadata("3", "My saved draft")],"pending_invitation_count":0}), None, None));
        pump_until(|| !state.get_team_page_loading());
        assert!(state.get_team_form_action().is_empty());
        let requests = transport.finish();
        let writes: Vec<Value> = requests.iter().filter(|request| request.starts_with("PATCH "))
            .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()).collect();
        assert_eq!(writes, vec![serde_json::json!({"name":"My saved draft","expected_version":"1"}), serde_json::json!({"name":"My saved draft","expected_version":"2"})]);
    }
    struct TeamFixture{_writer:client_state::tests::Fixture,lease:NamespaceLease,expected_join_failure:bool}
    impl Drop for TeamFixture{fn drop(&mut self){
        cancel_team_workers_for_retirement(&self.lease);
        let joined=join_team_workers();
        if !std::thread::panicking(){assert_eq!(joined.is_err(),self.expected_join_failure,"team fixture join outcome");}
    }}
    fn setup(url: &str) -> (TeamFixture, AppWindow, AppContext) {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = client_state::tests::Fixture::new(false, false);
        let session = Arc::new(SessionManager::new(Arc::new(api::test_support::MemoryRefreshTokenStore::default())));
        let scope = session.install_tokens_for_user(&TokenSet {
            access_token: "fixture-access".into(), access_expires_in_seconds: 1800,
            refresh_token: "fixture-refresh".into(), refresh_expires_at: "2099-01-01T00:00:00Z".into(),
            token_type: "X-Token".into(),
        }, USER).unwrap();
        let lease = fixture.lease(USER, scope.auth_epoch, 1);
        fixture.activate(lease.clone()).unwrap();
        let activity = UserActivityGate::default();
        activity.activate(lease.clone()).unwrap();
        let active = Arc::new(Mutex::new(Some(lease)));
        let client = ApiClient::new(ApiClientConfig {
            base_url: reqwest::Url::parse(url).unwrap(), app_version: "999.0.0".into(),
            timeout: Duration::from_secs(2),
        }, DeviceIdentity { id: "fixture-device".into(), name: "fixture".into(), platform: "macos".into() }, session).unwrap();
        client.bind_user_work(UserWorkAdmission::new(active.clone(), activity.clone())).unwrap();
        let billing = Arc::new(BillingContextManager::with_upgrade_latch(client.upgrade_latch().clone()));
        billing.bind_authenticated_session(scope.clone()).unwrap();
        let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":USER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill","manage_group","manage_members","manage_invitations"],
            "membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":GROUP,"name":"Fixture","group_status":"active","role":"owner",
                "member_id":null,"relationship_status":null,"readable_context":true,"selectable":true,
                "group_version":"1","membership_version":null,
                "capabilities":["bill","manage_group","manage_members","manage_invitations"],"quota":null}
        })).unwrap();
        let ticket = billing.begin_switch(&scope, "fixture-device", GROUP, PreviousBillingAuthority::StillValid).unwrap();
        let confirmation = billing.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot).unwrap();
        billing.publish_persisted(ticket, confirmation);
        let context = AppContext {
            backend: Some(Arc::new(BackendRuntime { api: client })),
            active_namespace: active, user_activity: activity, billing_context: billing,
            current_user_id: Arc::new(Mutex::new(Some(USER.into()))), ..Default::default()
        };
        let lease=context.active_namespace.lock().unwrap().clone().unwrap();
        let persistence=PrivatePersistence::for_test((*fixture).clone(),lease.clone(),context.user_activity.clone(),context.backend.as_ref().unwrap().api.upgrade_latch().clone());
        context.store.borrow_mut().private_persistence=Some(persistence);
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_logged_in(true);
        app.global::<AppState>().set_profile_open(true);
        app.global::<AppState>().set_account_center_section("accounts-teams".into());
        wire_team_callbacks(&app, context.clone());
        (TeamFixture{_writer:fixture,lease,expected_join_failure:false}, app, context)
    }
    fn pump_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !predicate() && Instant::now() < deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(predicate(), "team UI completion did not reach the required state");
    }
    fn respond(stream: &mut std::net::TcpStream, status: &str, data: serde_json::Value, code: Option<&str>) {
        let error = code.map(|code| serde_json::json!({"code":code,"message":"private diagnostic must not be shown","details":null}));
        let body = serde_json::json!({"request_id":"fixture","data":data,"error":error,"meta":{"next_cursor":null}}).to_string();
        write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    }
    fn accept_until(listener: &std::net::TcpListener) -> Option<std::net::TcpStream> {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return Some(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return None,
                Err(error) => panic!("fixture accept: {error}"),
            }
        }
    }

    fn controlled_response(status:u16,data:Value,cursor:Option<&str>,code:Option<&str>)->String{
        let body=serde_json::json!({"request_id":"fixture","data":data,"meta":{"next_cursor":cursor},
            "error":code.map(|code|serde_json::json!({"code":code,"message":"private upstream diagnostic","details":null}))}).to_string();
        format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())
    }
    fn member_page(version:&str,cursor:Option<&str>)->String{
        controlled_response(200,serde_json::json!({"items":[{
            "member_id":MEMBER,"user_id":"44444444-4444-4444-8444-444444444444","display_name":"Fixture member",
            "email_masked":"m***@example.com","status":"active","monthly_limit":"7",
            "quota":{"period_start":"2026-09-01T00:00:00Z","period_end":"2026-10-01T00:00:00Z",
            "monthly_limit":"7","settled":"1","reserved":"0","remaining":"6"},
            "joined_at":"2026-09-01T00:00:00Z","version":version}]}),cursor,None)
    }
    fn pump_for(duration:Duration){
        let until=Instant::now()+duration;
        while Instant::now()<until{
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();std::thread::sleep(Duration::from_millis(2));
        }
    }
    struct UiProgressRelease(Option<std::thread::JoinHandle<bool>>);
    impl UiProgressRelease{
        fn start(progress:mpsc::Receiver<()>,release:mpsc::Sender<()>)->Self{
            Self(Some(std::thread::spawn(move||{
                let progressed=progress.recv_timeout(Duration::from_millis(250)).is_ok();
                let _=release.send(());progressed
            })))
        }
        fn finish(mut self)->bool{self.0.take().unwrap().join().expect("fixture release panicked")}
    }
    impl Drop for UiProgressRelease{fn drop(&mut self){if let Some(worker)=self.0.take(){let joined=worker.join();if !std::thread::panicking(){assert!(joined.is_ok());}}}}
    struct ControlledTeamTransport {
        url:String, seen:mpsc::Receiver<usize>, replies:Vec<Option<mpsc::Sender<String>>>,
        stop:Arc<AtomicBool>, handle:Option<std::thread::JoinHandle<Vec<String>>>,
    }
    impl ControlledTeamTransport {
        fn new(slots:usize)->Self {
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url=format!("http://{}/",listener.local_addr().unwrap());
            let stop=Arc::new(AtomicBool::new(false)); let worker_stop=stop.clone();
            let (seen_tx,seen)=mpsc::channel(); let mut replies=Vec::new(); let mut receivers=Vec::new();
            for _ in 0..slots { let(tx,rx)=mpsc::channel(); replies.push(Some(tx));receivers.push(Some(rx)); }
            // A preconnect with no HTTP request must not consume a controlled response slot.
            let response_slots=Arc::new(Mutex::new((0usize,receivers)));
            let handle=std::thread::spawn(move || {
                let deadline=Instant::now()+Duration::from_secs(12); let mut children=Vec::new();
                while !worker_stop.load(Ordering::Acquire) && Instant::now()<deadline {
                    match listener.accept() {
                        Ok((mut stream,_)) => {
                            let seen_tx=seen_tx.clone();let response_slots=response_slots.clone();
                            children.push(std::thread::spawn(move || {
                                stream.set_nonblocking(false).unwrap();
                                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                                let mut bytes=Vec::new();let mut block=[0u8;1024];
                                let header_end=loop{
                                    if let Some(end)=bytes.windows(4).position(|value|value==b"\r\n\r\n"){
                                        assert!(end+4<=16384,"bounded team headers exceeded");break end+4;
                                    }
                                    assert!(bytes.len()<16384,"bounded team headers exceeded");
                                    let count=match stream.read(&mut block){
                                        Ok(count)=>count,
                                        Err(error) if bytes.is_empty() && matches!(error.kind(),std::io::ErrorKind::TimedOut|std::io::ErrorKind::WouldBlock)=>return None,
                                        Err(error)=>panic!("incomplete team fixture headers: {error}"),
                                    };
                                    if count==0{assert!(bytes.is_empty(),"incomplete team fixture headers");return None;}
                                    bytes.extend_from_slice(&block[..count]);
                                };
                                let header=std::str::from_utf8(&bytes[..header_end]).unwrap();
                                let lengths=header.lines().filter_map(|line|line.split_once(':'))
                                    .filter(|(name,_)|name.eq_ignore_ascii_case("content-length"))
                                    .map(|(_,value)|value.trim().parse::<usize>().unwrap()).collect::<Vec<_>>();
                                assert!(lengths.len()<=1,"duplicate fixture content length");
                                assert!(!header.lines().filter_map(|line|line.split_once(':')).any(|(name,_)|name.eq_ignore_ascii_case("transfer-encoding")),"fixture expects bounded content length");
                                let length=lengths.first().copied().unwrap_or(0);
                                assert!(length<=16384,"bounded team body exceeded");
                                let total=header_end.checked_add(length).unwrap();
                                assert!(bytes.len()<=total,"unexpected pipelined fixture bytes");
                                while bytes.len()<total{
                                    let remaining=(total-bytes.len()).min(block.len());
                                    let count=stream.read(&mut block[..remaining]).expect("incomplete team fixture body");
                                    assert!(count>0,"incomplete team fixture body");bytes.extend_from_slice(&block[..count]);
                                }
                                let(index,reply)={let mut slots=response_slots.lock().unwrap();let index=slots.0;
                                    slots.0=slots.0.checked_add(1).unwrap();(index,slots.1.get_mut(index).and_then(Option::take))};
                                seen_tx.send(index).unwrap();
                                let value=reply.and_then(|rx|rx.recv_timeout(Duration::from_secs(5)).ok())
                                    .unwrap_or_else(||controlled_response(400,Value::Null,None,Some("fixture_refused")));
                                let _=stream.write_all(value.as_bytes());
                                Some(String::from_utf8(bytes).unwrap())
                            }));
                        }
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                        Err(_)=>panic!("fixture listener failed"),
                    }
                }
                let mut requests=Vec::new();let mut failed=false;
                for child in children {match child.join(){Ok(Some(request))=>requests.push(request),Ok(None)=>{},Err(_)=>failed=true}}
                assert!(!failed,"fixture connection panicked");requests
            });
            Self{url,seen,replies,stop,handle:Some(handle)}
        }
        fn wait(&self){self.seen.recv_timeout(Duration::from_secs(4)).expect("team request was not dispatched");}
        fn reply(&mut self,index:usize,response:String){self.replies[index].take().unwrap().send(response).unwrap();}
        fn finish(mut self)->Vec<String>{
            self.stop.store(true,Ordering::Release);for reply in &mut self.replies{reply.take();}
            self.handle.take().unwrap().join().expect("fixture transport panicked")
        }
    }
    impl Drop for ControlledTeamTransport {
        fn drop(&mut self){
            self.stop.store(true,Ordering::Release);for reply in &mut self.replies{reply.take();}
            if let Some(handle)=self.handle.take(){let joined=handle.join();if !std::thread::panicking(){assert!(joined.is_ok(),"fixture transport panicked");}}
        }
    }
    struct JoinedTrip(Option<std::thread::JoinHandle<()>>);
    impl JoinedTrip {
        fn start(latch:UpgradeLatch)->Self {
            let observed=latch.clone();
            let trip=Self(Some(std::thread::spawn(move||latch.trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())}))));
            let deadline=Instant::now()+Duration::from_secs(3);
            while !observed.is_tripped() && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(1));}
            assert!(observed.is_tripped(),"upgrade admission did not close");trip
        }
        fn join(mut self){self.0.take().unwrap().join().expect("fixture trip panicked");}
    }
    impl Drop for JoinedTrip {
        fn drop(&mut self){if let Some(handle)=self.0.take(){let joined=handle.join();if !std::thread::panicking(){assert!(joined.is_ok(),"fixture trip panicked");}}}
    }
    #[test]
    fn core_team_older_same_page_completion_cannot_clear_newer_busy_or_cursor(){
        let mut transport=ControlledTeamTransport::new(2);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("first-cursor".into());transport.wait();
        state.invoke_load_team_members("second-cursor".into());transport.wait();
        transport.reply(0,member_page("1",Some("stale-next")));pump_for(Duration::from_millis(60));
        assert!(state.get_team_page_loading(),"older same-page response released newer request busy");
        assert_eq!(state.get_team_members().row_count(),0);assert!(state.get_team_members_next_cursor().is_empty());
        transport.reply(1,member_page("2",Some("current-next")));
        pump_until(||state.get_team_members().row_count()==1 && !state.get_team_page_loading());
        assert_eq!(state.get_team_members().row_data(0).unwrap().version,"2");assert_eq!(state.get_team_members_next_cursor(),"current-next");
        let requests=transport.finish();assert_eq!(requests.len(),2);
        assert!(requests[0].contains("cursor=first-cursor"));assert!(requests[1].contains("cursor=second-cursor"));
        assert!(requests.iter().all(|request|!request.to_ascii_lowercase().contains("x-account-group-id:")));
    }
    #[test]
    fn core_team_sibling_page_completion_keeps_current_pending_work_busy(){
        let mut transport=ControlledTeamTransport::new(2);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("".into());transport.wait();state.invoke_load_team_invitations("".into());transport.wait();
        transport.reply(0,member_page("2",None));pump_until(||state.get_team_members().row_count()==1);
        assert!(state.get_team_page_loading(),"member response cleared still-pending invitation page");
        transport.reply(1,controlled_response(200,serde_json::json!({"items":[]}),Some("invitations-next"),None));
        pump_until(||!state.get_team_page_loading());assert_eq!(state.get_team_invitations_next_cursor(),"invitations-next");
        assert_eq!(transport.finish().len(),2);
    }
    #[test]
    fn core_team_newer_page_response_wins_even_when_older_http_finishes_last(){
        let mut transport=ControlledTeamTransport::new(2);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("first".into());transport.wait();state.invoke_load_team_members("second".into());transport.wait();
        transport.reply(1,member_page("2",Some("current")));pump_until(||state.get_team_members().row_count()==1);
        assert!(!state.get_team_page_loading(),"superseded page must not keep current page busy");
        transport.reply(0,member_page("1",Some("stale")));pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_members().row_data(0).unwrap().version,"2");assert_eq!(state.get_team_members_next_cursor(),"current");
        assert_eq!(transport.finish().len(),2);
    }
    #[test]
    fn core_team_exact_upgrade_entry_never_changes_page_error_or_busy(){
        let transport=ControlledTeamTransport::new(0);let(_fixture,app,context)=setup(&transport.url);let state=app.global::<AppState>();
        context.backend.as_ref().unwrap().api.upgrade_latch().trip(RequiredUpgrade{minimum_version:None});
        state.set_team_page_error("upgrade boundary".into());state.set_team_page_loading(false);state.set_team_name_input("".into());
        state.invoke_load_team_members("".into());state.invoke_rename_team();state.invoke_update_team_member(MEMBER.into(),"1".into(),"suspend".into());
        assert_eq!(state.get_team_page_error(),"upgrade boundary");assert!(!state.get_team_page_loading());assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_team_late_http_after_upgrade_cannot_publish_rows_error_or_busy(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("".into());transport.wait();
        let trip=JoinedTrip::start(context.backend.as_ref().unwrap().api.upgrade_latch().clone());
        state.set_team_page_error("upgrade boundary".into());state.set_team_page_loading(true);
        transport.reply(0,member_page("2",Some("private-next")));trip.join();pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_members().row_count(),0);assert!(state.get_team_members_next_cursor().is_empty());
        assert_eq!(state.get_team_page_error(),"upgrade boundary");assert!(state.get_team_page_loading());assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_team_reauth_code_uses_original_identity_and_server_cooldown(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.set_password_set(false);state.set_email_bound(true);state.set_team_tab("reauth".into());slint::platform::update_timers_and_animations();
        state.invoke_request_team_reauth_code();transport.wait();
        transport.reply(0,controlled_response(200,serde_json::json!({"email_masked":"a***@example.com","expires_in_seconds":300,"resend_after_seconds":60}),None,None));
        pump_until(||!state.get_team_reauth_code_busy() && state.get_team_reauth_countdown()>0);
        state.invoke_request_team_reauth_code();assert!(state.get_team_reauth_countdown()>0);
        let requests=transport.finish();assert_eq!(requests.len(),1);
        assert!(requests[0].starts_with("POST /v1/account/reauth/code "));
        assert!(!requests[0].to_ascii_lowercase().contains("x-account-group-id:"));
    }
    #[test]
    fn core_team_receive_before_worker_exit_does_not_block_ui_or_publish_early(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        let(sent_tx,sent_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();
        TEAM_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{sent_tx.send(()).unwrap();let _=release_rx.recv_timeout(Duration::from_secs(3));})));
        state.invoke_load_team_members("".into());transport.wait();transport.reply(0,member_page("2",None));sent_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let(progress_tx,progress_rx)=mpsc::channel();let release=UiProgressRelease::start(progress_rx,release_tx);
        slint::Timer::single_shot(Duration::from_millis(60),move||{let _=progress_tx.send(());});
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));slint::platform::update_timers_and_animations();
        let early_rows=state.get_team_members().row_count();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));slint::platform::update_timers_and_animations();
        assert!(release.finish(),"poll joined an unfinished worker on the UI thread");assert_eq!(early_rows,0);
        pump_until(||state.get_team_members().row_count()==1);assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_team_weak_window_loss_does_not_join_live_worker_on_ui(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,_context)=setup(&transport.url);
        let(sent_tx,sent_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();
        TEAM_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{sent_tx.send(()).unwrap();let _=release_rx.recv_timeout(Duration::from_secs(3));})));
        app.global::<AppState>().invoke_load_team_members("".into());transport.wait();transport.reply(0,member_page("2",None));sent_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let(progress_tx,progress_rx)=mpsc::channel();let release=UiProgressRelease::start(progress_rx,release_tx);
        slint::Timer::single_shot(Duration::from_millis(60),move||{let _=progress_tx.send(());});drop(app);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));slint::platform::update_timers_and_animations();
        assert!(release.finish(),"weak-window disposal joined an unfinished worker on UI");
        pump_for(Duration::from_millis(60));assert_eq!(transport.finish().len(),1);
    }
    // Break caught: a version error is displayed without reading the authoritative affected page.
    #[test]
    fn core_team_reaped_panicking_worker_remains_failure_at_empty_shutdown(){
        let(mut fixture,app,context)=setup("http://127.0.0.1:9/");fixture.expected_join_failure=true;
        run_team_job_handled::<()>(&app,context,None,true,|_,_,_|panic!("controlled team worker panic"),|_,_,_|panic!("panic cannot become success"),
            Box::new(|app,_,_|app.global::<AppState>().set_team_page_error("interrupted fixture".into())));
        pump_until(||TEAM_JOIN_FAILED.with(|failed|failed.get()));
        assert!(TEAM_WORKERS.with(|workers|workers.borrow().is_empty()));assert!(shutdown_team_workers().is_err());assert!(join_team_workers().is_err());
    }
    #[test]
    fn core_team_shutdown_joins_real_completed_worker_without_dispatching_its_ui(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("".into());transport.wait();transport.reply(0,member_page("2",Some("private-next")));
        // No timer is dispatched: shutdown itself owns and joins the actual HTTP worker.
        shutdown_team_workers().unwrap();assert!(TEAM_WORKERS.with(|workers|workers.borrow().is_empty()));assert!(TEAM_BUSY.with(|busy|busy.borrow().is_empty()));
        state.set_team_page_error("shutdown boundary".into());pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_members().row_count(),0);assert_eq!(state.get_team_page_error(),"shutdown boundary");assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_team_retired_binding_late_page_cannot_publish_into_replacement(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("".into());transport.wait();let lease=context.active_namespace.lock().unwrap().clone().unwrap();
        cancel_team_workers_for_retirement(&lease);context.store.borrow_mut().private_persistence=None;*context.active_namespace.lock().unwrap()=None;
        state.set_team_page_error("replacement boundary".into());state.set_team_page_loading(false);
        transport.reply(0,member_page("2",Some("private-next")));join_team_workers().unwrap();pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_page_error(),"replacement boundary");assert!(!state.get_team_page_loading());assert_eq!(state.get_team_members().row_count(),0);
        assert!(state.get_team_members_next_cursor().is_empty());assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_team_page_completion_cannot_release_pending_mutation_busy(){
        let mut transport=ControlledTeamTransport::new(2);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.invoke_load_team_members("".into());transport.wait();state.invoke_update_team_member(MEMBER.into(),"1".into(),"suspend".into());transport.wait();
        transport.reply(0,member_page("2",None));pump_until(||state.get_team_members().row_count()==1);
        assert!(state.get_team_page_loading(),"page response released pending member mutation");
        transport.reply(1,controlled_response(403,Value::Null,None,Some("group_frozen")));pump_until(||!state.get_team_page_loading());
        let requests=transport.finish();assert_eq!(requests.len(),2);assert!(requests[1].starts_with(&format!("PATCH /v1/account-groups/{GROUP}/members/{MEMBER} ")));
    }
    #[test]
    fn core_team_reauth_mode_change_rejects_old_method_receipt(){
        let mut transport=ControlledTeamTransport::new(1);let(_fixture,app,_context)=setup(&transport.url);let state=app.global::<AppState>();
        state.set_password_set(true);state.set_email_bound(true);state.set_team_tab("reauth".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.get_team_reauth_mode(),"current_password");
        state.set_team_reauth_password("original secret".into());state.invoke_confirm_team_reauth();transport.wait();
        state.set_team_reauth_mode("email_code".into());pump_for(Duration::from_millis(20));
        transport.reply(0,controlled_response(200,serde_json::json!({"user_id":USER,"reauthenticated_at":"2026-09-07T00:00:00Z","expires_at":"2099-01-01T00:00:00Z"}),None,None));
        join_team_workers().unwrap();pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_reauth_mode(),"email_code");assert!(state.get_team_reauth_status().is_empty());assert!(state.get_team_reauth_password().is_empty());
        assert_eq!(transport.finish().len(),1);
    }

    fn held_member_mutation_refreshes_current_page(conflict:bool){
        let mut transport=ControlledTeamTransport::new(4);
        let(_fixture,app,context)=setup(&transport.url);let state=app.global::<AppState>();
        let original_scope=context.current_account_session_scope().unwrap();
        let original_billing=context.billing_context.confirmed_scope().unwrap();
        let original_lease=context.store.borrow().private_persistence.as_ref().unwrap().lease().clone();
        state.invoke_load_team_members("original-cursor".into());transport.wait();
        transport.reply(0,member_page("1",Some("next-original")));
        pump_until(||state.get_team_members().row_count()==1);
        state.invoke_update_team_member(MEMBER.into(),"1".into(),"suspend".into());transport.wait();
        state.invoke_load_team_members("current-cursor".into());transport.wait();
        transport.reply(2,member_page("2",Some("next-current")));
        pump_until(||state.get_team_members().row_data(0).is_some_and(|row|row.version=="2"));
        assert!(state.get_team_page_loading(),"the original mutation is still pending");
        let response=if conflict{
            controlled_response(409,Value::Null,None,Some("membership_version_conflict"))
        }else{
            let response=member_page("3",None);
            let envelope:Value=serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            controlled_response(200,envelope["data"]["items"][0].clone(),None,None)
        };
        transport.reply(1,response);
        let seen=Cell::new(false);
        pump_until(||{
            if !seen.get(){match transport.seen.try_recv(){
                Ok(index)=>{assert_eq!(index,3);seen.set(true);},
                Err(mpsc::TryRecvError::Empty)=>{},
                Err(mpsc::TryRecvError::Disconnected)=>panic!("refresh transport disconnected"),
            }}
            seen.get()
        });
        transport.reply(3,member_page("4",Some("fresh-next")));
        pump_until(||!state.get_team_page_loading() && state.get_team_members().row_data(0).is_some_and(|row|row.version=="4"));
        assert_eq!(state.get_team_members_next_cursor(),"fresh-next");
        assert_eq!(context.current_account_session_scope().as_ref(),Some(&original_scope));
        assert_eq!(context.billing_context.confirmed_scope().as_ref(),Some(&original_billing));
        assert_eq!(context.store.borrow().private_persistence.as_ref().unwrap().lease(),&original_lease);
        let requests=transport.finish();assert_eq!(requests.len(),4);
        let mutations=requests.iter().filter(|request|request.starts_with("PATCH ")).collect::<Vec<_>>();
        assert_eq!(mutations.len(),1,"a late mutation must never be replayed");
        assert!(mutations[0].starts_with(&format!("PATCH /v1/account-groups/{GROUP}/members/{MEMBER} ")));
        let body:Value=serde_json::from_str(mutations[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body,serde_json::json!({"action":"suspend","expected_version":"1"}));
        let mutation_headers=mutations[0].split_once("\r\n\r\n").unwrap().0;
        let keys=mutation_headers.lines().filter_map(|line|line.split_once(':')).filter(|(name,_)|name.eq_ignore_ascii_case("idempotency-key")).map(|(_,value)|value.trim()).collect::<Vec<_>>();
        assert_eq!(keys.len(),1);assert!(Uuid::parse_str(keys[0]).is_ok());
        let reads=requests.iter().filter(|request|request.starts_with("GET ")).collect::<Vec<_>>();
        assert_eq!(reads.len(),3);
        assert_eq!(reads.iter().filter(|request|request.lines().next().unwrap().contains("cursor=original-cursor")).count(),1);
        assert_eq!(reads.iter().filter(|request|request.lines().next().unwrap().contains("cursor=current-cursor")).count(),2,
            "late mutation must refresh the latest page, not its captured old cursor");
        for request in &requests{
            let headers=request.split_once("\r\n\r\n").unwrap().0.to_ascii_lowercase();
            assert!(headers.contains("x-token: fixture-access"));
            assert!(!headers.contains("x-account-group-id:"),"existing team resources remain real-user identity requests");
            assert!(request.lines().next().unwrap().contains(&format!("/v1/account-groups/{GROUP}/members")));
        }
    }
    #[test]
    fn core_team_late_successful_mutation_refreshes_current_page(){held_member_mutation_refreshes_current_page(false);}
    #[test]
    fn core_team_late_conflict_mutation_refreshes_current_page(){held_member_mutation_refreshes_current_page(true);}
    #[test]
    fn core_team_worker_panic_closes_admission_and_cancels_pending_work(){
        let mut transport=ControlledTeamTransport::new(1);let(mut fixture,app,context)=setup(&transport.url);
        fixture.expected_join_failure=true;let state=app.global::<AppState>();
        state.invoke_load_team_members("held-cursor".into());transport.wait();
        run_team_job_handled::<()>(&app,context.clone(),None,false,|_,_,_|panic!("controlled team panic"),
            |_,_,_|panic!("panic cannot succeed"),Box::new(|_,_,_|{}));
        pump_until(||TEAM_JOIN_FAILED.with(|failed|failed.get()));
        assert!(TEAM_SHUTDOWN.with(|closed|closed.get()),"a reaped worker panic must close new admission");
        assert!(TEAM_WORKERS.with(|workers|!workers.borrow().is_empty()
            && workers.borrow().iter().all(|worker|worker.cancel.load(Ordering::Acquire))));
        let before_rows=state.get_team_members().row_count();
        state.set_team_page_error("closed boundary".into());state.set_team_page_loading(false);
        state.invoke_load_team_members("must-not-dispatch".into());
        assert_eq!(state.get_team_page_error(),"closed boundary");assert!(!state.get_team_page_loading());
        transport.reply(0,member_page("2",Some("private-next")));assert!(join_team_workers().is_err());
        pump_for(Duration::from_millis(60));
        assert_eq!(state.get_team_members().row_count(),before_rows);assert_eq!(state.get_team_page_error(),"closed boundary");
        assert!(shutdown_team_workers().is_err());assert!(TEAM_WORKERS.with(|workers|workers.borrow().is_empty()));
        assert_eq!(transport.finish().len(),1);
    }

    #[test]
    fn core_team_undelivered_real_job_timer_is_safe_at_ui_thread_exit(){
        // Run this exact test alone for RED: a panic in TLS destruction can abort the process.
        // This is an actual Slint timer holding an actual completed TeamApi job, not a fake Drop.
        let ui=std::thread::spawn(||{
            let mut transport=ControlledTeamTransport::new(1);
            let(fixture,app,context)=setup(&transport.url);let state=app.global::<AppState>();
            state.invoke_load_team_members("undelivered-cursor".into());transport.wait();
            transport.reply(0,member_page("2",Some("private-next")));
            // Real network worker and fixture transport finish independently of the UI timer.
            join_team_workers().unwrap();assert_eq!(transport.finish().len(),1);
            assert!(TEAM_WORKERS.with(|workers|workers.borrow().is_empty()));
            assert_eq!(TEAM_BUSY.with(|busy|busy.borrow().len()),1);
            assert_eq!(state.get_team_members().row_count(),0);
            drop(state);drop(app);drop(context);drop(fixture);
            // Deliberately no timer update/shutdown clear: the live Slint poll owns the job
            // until this thread's real TLS destructors run, after TEAM_BUSY was destroyed.
        });
        ui.join().expect("team UI thread exit panicked");
    }
    #[test]
    fn core_team_ui_member_conflict_refreshes_without_replaying_mutation() {
        let (listener, url) = listener();
        let (_fixture, app, _context) = setup(&url);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let transport = JoinedFixtureTransport::spawn(move || {
            let mut stream = accept_until(&listener).unwrap();
            captured.lock().unwrap().push(read_request(&mut stream).lines().next().unwrap().to_owned());
            respond(&mut stream, "409 Conflict", serde_json::Value::Null, Some("membership_version_conflict"));
            drop(stream);
            if let Some(mut stream) = accept_until(&listener) {
                captured.lock().unwrap().push(read_request(&mut stream).lines().next().unwrap().to_owned());
                respond(&mut stream, "200 OK", serde_json::json!({"items":[{
                    "member_id":MEMBER,"user_id":"44444444-4444-4444-8444-444444444444",
                    "display_name":"Updated member","email_masked":"m***@example.com","status":"suspended",
                    "monthly_limit":"7","quota":{"period_start":"2026-09-01T00:00:00Z","period_end":"2026-10-01T00:00:00Z",
                    "monthly_limit":"7","settled":"1","reserved":"0","remaining":"6"},
                    "joined_at":"2026-09-01T00:00:00Z","version":"2"
                }]}), None);
            }
        });
        app.global::<AppState>().invoke_update_team_member(MEMBER.into(), "1".into(), "suspend".into());
        pump_until(|| requests.lock().unwrap().len() == 2);
        transport.join();
        let lines = requests.lock().unwrap();
        assert_eq!(lines.len(), 2, "a conflict must refresh, not silently retain the stale row");
        assert!(lines[0].contains(&format!("/account-groups/{GROUP}/members/{MEMBER} ")));
        assert!(lines[1].starts_with(&format!("GET /v1/account-groups/{GROUP}/members?")));
        drop(lines);
        pump_until(|| app.global::<AppState>().get_team_members().row_count() == 1);
        assert_eq!(app.global::<AppState>().get_team_members().row_data(0).unwrap().version.as_str(), "2");
        assert!(!app.global::<AppState>().get_team_page_error().is_empty());
    }
    // Break caught: closing the form leaves password/code in global UI and accepts a late proof.
    #[test]
    fn core_team_ui_reauth_close_clears_secrets_and_discards_late_proof() {
        let (listener, url) = listener();
        let (_fixture, app, _context) = setup(&url);
        let state = app.global::<AppState>();
        state.set_password_set(true);
        state.set_email_bound(true);
        state.set_team_tab("reauth".into());
        // Opening the visible form deliberately initializes its token outside
        // the property callback's short guard. Dispatch that timer before typing.
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.get_team_reauth_mode(),"current_password");
        state.set_team_reauth_password("original-secret".into());
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let transport = JoinedFixtureTransport::spawn(move || {
            let mut stream = accept_until(&listener).unwrap();
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /v1/account/reauth "));
            assert!(!request.to_ascii_lowercase().contains("x-account-group-id:"));
            let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(body, serde_json::json!({"current_password":"original-secret"}));
            seen_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(4)).unwrap();
            respond(&mut stream, "200 OK", serde_json::json!({"user_id":USER,"reauthenticated_at":"2026-09-07T00:00:00Z","expires_at":"2099-01-01T00:00:00Z"}), None);
        });
        state.invoke_confirm_team_reauth();
        seen_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        state.set_team_reauth_password("typed-after-submit".into());
        state.set_team_reauth_code("654321".into());
        state.set_profile_open(false);
        slint::platform::update_timers_and_animations();
        release_tx.send(()).unwrap();
        transport.join();
        for _ in 0..5 { i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100)); slint::platform::update_timers_and_animations(); }
        assert!(state.get_team_reauth_password().is_empty());
        assert!(state.get_team_reauth_code().is_empty());
        assert!(state.get_team_reauth_status().is_empty());
    }
    // Break caught: the callbacks exist but no reachable account-center verification action exists.
    #[test]
    fn core_team_ui_reauth_controls_are_visible_in_the_existing_panel() {
        use i_slint_backend_testing::ElementHandle;
        let (_fixture, app, _context) = setup("http://127.0.0.1:9");
        let state = app.global::<AppState>();
        state.set_password_set(true);
        state.set_email_bound(true);
        state.set_team_tab("reauth".into());
        app.show().unwrap();
        assert!(ElementHandle::find_by_element_id(&app, "TeamReauthPanel::reauth-confirm").next().is_some());
        assert!(ElementHandle::find_by_element_id(&app, "TeamReauthPanel::reauth-password").next().is_some());
        state.set_profile_open(false);
    }

    #[test]
    fn core_team_ui_email_only_submits_code_not_hidden_password() {
        let (listener, url) = listener();
        let (_fixture, app, _context) = setup(&url);
        let state = app.global::<AppState>();
        state.set_password_set(false); state.set_email_bound(true);
        state.set_team_tab("reauth".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.get_team_reauth_mode(),"email_code");
        state.set_team_reauth_password("hidden-must-not-win".into());
        state.set_team_reauth_code("123456".into());
        let transport = JoinedFixtureTransport::spawn(move || {
            let mut stream = accept_until(&listener).unwrap();
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /v1/account/reauth "));
            let headers = request.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
            assert!(!headers.contains("x-account-group-id:") && !headers.contains("idempotency-key:"));
            let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(body, serde_json::json!({"email_code":"123456"}));
            respond(&mut stream, "200 OK", serde_json::json!({"user_id":USER,"reauthenticated_at":"2026-09-07T00:00:00Z","expires_at":"2099-01-01T00:00:00Z"}), None);
        });
        state.invoke_confirm_team_reauth();
        transport.join();
        pump_until(|| !state.get_team_reauth_status().is_empty());
        assert!(state.get_team_reauth_password().is_empty() && state.get_team_reauth_code().is_empty());
        state.set_profile_open(false);
    }
    #[test]
    fn core_team_ui_unavailable_method_never_starts_identity_transport() {
        let (listener, url) = listener();
        let (_fixture, app, _context) = setup(&url);
        let state = app.global::<AppState>();
        state.set_password_set(false); state.set_email_bound(false);
        state.set_team_tab("reauth".into());
        slint::platform::update_timers_and_animations();
        let transport = JoinedFixtureTransport::spawn(move || {
            assert!(accept_until(&listener).is_none(), "unavailable methods must be refused before HTTP");
        });
        state.invoke_confirm_team_reauth();
        state.invoke_request_team_reauth_code();
        transport.join();
        state.set_profile_open(false);
    }
}

fn team_decimal(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(transition_error("请输入非负整数额度"));
    }
    value.parse::<i64>().map(|value| value.to_string()).map_err(|_| transition_error("额度超出支持范围"))
}
fn team_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if !(1..=128).contains(&value.chars().count()) { return Err(transition_error("团队名称应为 1–128 个字符")); }
    Ok(value.into())
}
fn team_amount(choice: &AccountGroupChoice, snapshot: Option<&AccountSnapshot>) -> String {
    if choice.has_capability(KnownCapability::ReadOwnQuota) {
        return presentation::own_quota(choice, snapshot).map(|quota| format!("剩余额度 {}", quota.remaining)).unwrap_or_default();
    }
    snapshot.filter(|snapshot| snapshot.billing_group.group_id == choice.group_id
        && choice.has_capability(KnownCapability::ReadGroupFinance))
        .and_then(|snapshot| snapshot.credits.as_ref()).map(|credits| format!("{} 积分", credits.available)).unwrap_or_default()
}
fn team_card(choice: &AccountGroupChoice, current: Option<&BillingScope>, snapshot: Option<&AccountSnapshot>) -> AccountGroupCardView {
    AccountGroupCardView {
        group_id: choice.group_id.clone().into(), name: choice.name.clone().into(),
        group_status: choice.group_status.clone().into(),
        role_label: (if choice.role == "owner" { "主账号" } else { "成员" }).into(),
        relationship_status: choice.relationship_status.clone().unwrap_or_default().into(),
        entitlement_label: (if choice.readable_context && !choice.selectable { "只读" } else { "" }).into(),
        amount_label: team_amount(choice, snapshot).into(),
        selected: current.is_some_and(|scope| scope.request.account_group_id == choice.group_id),
        selectable: choice.selectable, read_only: !choice.selectable, owned: choice.role == "owner",
    }
}
fn team_member_row(member: MemberView) -> TeamMemberRowView {
    TeamMemberRowView { member_id: member.member_id.into(), display_name: member.display_name.into(),
        email_masked: member.email_masked.into(), status: member.status.into(),
        monthly_limit: member.monthly_limit.into(), settled: member.quota.settled.into(),
        reserved: member.quota.reserved.into(), remaining: member.quota.remaining.into(),
        joined_at: member.joined_at.into(), version: member.version.into() }
}
fn team_invitation_row(invitation: InvitationView) -> TeamInvitationRowView {
    let actionable = presentation::invitation_actionable(&invitation.status, &invitation.expires_at);
    TeamInvitationRowView { invitation_id: invitation.invitation_id.into(), team_name: invitation.team_name.into(),
        owner_display_name: invitation.owner_display_name.into(), recipient_email_masked: invitation.recipient_email_masked.into(),
        monthly_limit: invitation.monthly_limit.into(), status: invitation.status.into(),
        expires_at: presentation::display_time(&invitation.expires_at).into(), version: invitation.version.into(), delivery_status: "".into(), actionable }
}
fn team_owner_invitation_row(invitation: OwnerInvitationView) -> TeamInvitationRowView {
    let actionable = presentation::invitation_actionable(&invitation.status, &invitation.expires_at);
    TeamInvitationRowView { invitation_id: invitation.invitation_id.into(), team_name: invitation.team_name.into(),
        owner_display_name: invitation.owner_display_name.into(), recipient_email_masked: invitation.recipient_email_masked.into(),
        monthly_limit: invitation.monthly_limit.into(), status: invitation.status.into(),
        expires_at: presentation::display_time(&invitation.expires_at).into(), version: invitation.version.into(), delivery_status: invitation.delivery_status.into(), actionable }
}
fn team_usage_row(usage: UsageEventView) -> TeamUsageRowView {
    // Deliberately discard user_id/email; the DTO itself rejects private content keys.
    TeamUsageRowView { usage_event_id: usage.usage_event_id.into(), member_display_name: usage.member.display_name.into(),
        occurred_at: presentation::display_time(&usage.occurred_at).into(), period_start: usage.period_start.into(), period_end: usage.period_end.into(),
        operation_kind: presentation::operation_label(&usage.operation_kind).into(), model_label: usage.model_label.into(),
        credit_amount: usage.credit_amount.into(), phase: presentation::phase_label(&usage.phase).into(), outcome: presentation::outcome_label(&usage.outcome).into() }
}
pub(super) fn render_team_context(app: &AppWindow, context: &AppContext) {
    let state = app.global::<AppState>();
    let current = context.billing_context.confirmed_scope();
    let snapshot = context.billing_context.confirmed_snapshot();
    let mut summary = PreparedUiProjection::default();
    presentation::prepare_summary(&mut summary, &context.team_groups.borrow(), snapshot.as_ref());
    summary.publish(&state);
    let choice = snapshot.as_ref().map(|snapshot| &snapshot.billing_group);
    let cap = |capability| choice.is_some_and(|choice| choice.has_capability(capability));
    state.set_selected_account_group_id(current.as_ref().map(|scope| scope.request.account_group_id.clone()).unwrap_or_default().into());
    state.set_account_group_read_only(snapshot.as_ref().is_some_and(|snapshot| snapshot.read_only));
    state.set_account_amount_label(choice.map(|choice| team_amount(choice, snapshot.as_ref())).unwrap_or_default().into());
    state.set_team_can_manage_group(cap(KnownCapability::ManageGroup));
    state.set_team_can_manage_members(cap(KnownCapability::ManageMembers));
    state.set_team_can_manage_invitations(cap(KnownCapability::ManageInvitations));
    state.set_team_can_read_usage(cap(KnownCapability::ReadGroupUsage));
    state.set_team_can_read_finance(cap(KnownCapability::ReadGroupFinance));
    state.set_team_can_leave(cap(KnownCapability::LeaveTeam));
    state.set_team_can_purchase(cap(KnownCapability::Purchase));
    state.set_team_can_redeem(cap(KnownCapability::Redeem));
    if state.get_team_form_action() != "rename" {
        state.set_team_name_input(choice.map(|choice| choice.name.clone()).unwrap_or_default().into());
    }
    state.set_account_groups(ModelRc::new(VecModel::from(context.team_groups.borrow().iter()
        .map(|choice| team_card(choice, current.as_ref(), snapshot.as_ref())).collect::<Vec<_>>())));
}
pub(super) fn clear_team_context(app: &AppWindow, context: &AppContext) {
    context.team_groups.borrow_mut().clear();
    let state = app.global::<AppState>();
    presentation::clear_forms_and_feedback(&state);
    state.set_team_members(ModelRc::new(VecModel::default()));
    state.set_team_invitations(ModelRc::new(VecModel::default()));
    state.set_pending_team_invitations(ModelRc::new(VecModel::default()));
    state.set_team_usage(ModelRc::new(VecModel::default()));
    state.set_team_members_next_cursor("".into()); state.set_team_invitations_next_cursor("".into());
    state.set_pending_team_invitations_next_cursor("".into()); state.set_team_usage_next_cursor("".into());
    state.set_pending_team_invitation_count("0".into());
    state.set_team_invite_email("".into()); state.set_team_reauth_code("".into()); state.set_team_reauth_password("".into());
    render_team_context(app, context);
    let mut summary = PreparedUiProjection::default();
    presentation::prepare_summary(&mut summary, &[], None);
    summary.publish(&state);
}
#[derive(Clone)]
struct TeamCapture{
    persistence:PrivatePersistence,scope:SessionScope,billing:Option<BillingScope>,
}
impl TeamCapture{
    // Memory-only capture is also usable while constructing a zero-timer continuation.
    fn snapshot(context:&AppContext)->Result<Self,ApiError>{
        let persistence=context.store.borrow().private_persistence.clone().ok_or(ApiError::AuthenticationRequired)?;
        let scope=context.current_account_session_scope().ok_or(ApiError::AuthenticationRequired)?;
        if persistence.lease().auth_epoch!=scope.auth_epoch || persistence.lease().namespace.user_public_id()!=scope.owner_user_id{
            return Err(ApiError::AuthenticationRequired);
        }
        Ok(Self{persistence,scope,billing:None})
    }
    fn new(context:&AppContext)->Result<Self,ApiError>{
        let captured=Self::snapshot(context)?;
        if !captured.current(context){return Err(ApiError::AuthenticationRequired);}Ok(captured)
    }
    fn binding_matches(&self,context:&AppContext)->bool{
        context.store.borrow().private_persistence.as_ref().is_some_and(|value|value.lease()==self.persistence.lease())
            && context.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
    }
    fn namespace_current(&self,context:&AppContext)->bool{self.binding_matches(context) && self.persistence.is_current()}
    fn current(&self,context:&AppContext)->bool{
        !TEAM_SHUTDOWN.with(|closed|closed.get()) && self.namespace_current(context)
            && context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.scope))
            && self.billing.as_ref().is_none_or(|scope|context.billing_context.is_current(scope))
    }
    fn apply<R>(&self,context:&AppContext,apply:impl FnOnce()->R)->Option<R>{
        if !self.current(context){return None;}
        context.apply_user_completion(self.persistence.lease(),||{
            if !self.binding_matches(context) || self.billing.as_ref().is_some_and(|scope|!context.billing_context.is_current(scope)){return None;}
            Some(apply())
        }).ok().flatten()
    }
}
struct TeamWorker{id:Uuid,lease:NamespaceLease,cancel:Arc<std::sync::atomic::AtomicBool>,handle:std::thread::JoinHandle<()>}
struct TeamBusy{id:Uuid,capture:TeamCapture,current:Rc<dyn Fn()->bool>}
thread_local!{
    static TEAM_WORKERS:RefCell<Vec<TeamWorker>>=const{RefCell::new(Vec::new())};
    static TEAM_BUSY:Rc<RefCell<Vec<TeamBusy>>>=Rc::new(RefCell::new(Vec::new()));
    static TEAM_JOIN_FAILED:Cell<bool>=const{Cell::new(false)};
    static TEAM_SHUTDOWN:Cell<bool>=const{Cell::new(false)};
    static TEAM_GROUP_REQUEST:RefCell<Option<(NamespaceLease,Uuid)>>=const{RefCell::new(None)};
}
fn reap_team_workers(){
    let ready=TEAM_WORKERS.with(|workers|{
        let mut workers=workers.borrow_mut();let mut ready=Vec::new();let mut index=0;
        while index<workers.len(){if workers[index].handle.is_finished(){ready.push(workers.remove(index));}else{index+=1;}}ready
    });
    for worker in ready{if worker.handle.join().is_err(){fail_team_worker_lifecycle();}}
}
// Only called outside short completion/ordinary guards. Panic permanently closes this family.
fn fail_team_worker_lifecycle(){
    TEAM_JOIN_FAILED.with(|failed|failed.set(true));TEAM_SHUTDOWN.with(|closed|closed.set(true));
    cancel_team_workers_for_upgrade();
}
fn team_worker_pending(id:Uuid)->bool{TEAM_WORKERS.with(|workers|workers.borrow().iter().any(|worker|worker.id==id))}
fn join_team_workers()->Result<(),String>{
    let mut workers=TEAM_WORKERS.with(|workers|std::mem::take(&mut *workers.borrow_mut())).into_iter();
    while let Some(worker)=workers.next(){if worker.handle.join().is_err(){
        fail_team_worker_lifecycle();
        // These handles were moved out of the registry; they must also observe cancellation.
        for pending in workers.as_slice(){pending.cancel.store(true,std::sync::atomic::Ordering::Release);}
    }}
    if TEAM_JOIN_FAILED.with(|failed|failed.get()){Err("team worker panicked".into())}else{Ok(())}
}
pub(super) fn cancel_team_workers_for_retirement(lease:&NamespaceLease){
    TEAM_WORKERS.with(|workers|for worker in workers.borrow().iter().filter(|worker|&worker.lease==lease){worker.cancel.store(true,std::sync::atomic::Ordering::Release);});
}
pub(super) fn cancel_team_workers_for_upgrade(){
    TEAM_WORKERS.with(|workers|for worker in workers.borrow().iter(){worker.cancel.store(true,std::sync::atomic::Ordering::Release);});
}
/// Owning UI thread after event-loop exit; never call from a completion/activity guard.
pub(super) fn shutdown_team_workers()->Result<(),String>{
    TEAM_SHUTDOWN.with(|closed|closed.set(true));cancel_team_workers_for_upgrade();
    let joined=join_team_workers();TEAM_BUSY.with(|busy|busy.borrow_mut().clear());TEAM_GROUP_REQUEST.with(|request|request.borrow_mut().take());joined
}
fn poll_orphan_team_worker(id:Uuid){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_team_workers();if team_worker_pending(id){poll_orphan_team_worker(id);}
    });
}
fn team_busy_now(context:&AppContext)->bool{
    TEAM_BUSY.with(|busy|busy.borrow().iter().any(|entry|(entry.current)() && entry.capture.current(context)))
}
struct TeamJob<R>{
    id:Uuid,receiver:mpsc::Receiver<Result<R,ApiError>>,
    // The timer can outlive TEAM_BUSY TLS. Own the exact original set, never look it up in Drop.
    busy:Option<Rc<RefCell<Vec<TeamBusy>>>>,
}
impl<R> TeamJob<R>{
    fn release_busy(&mut self){
        if let Some(busy)=self.busy.take(){busy.borrow_mut().retain(|entry|entry.id!=self.id);}
    }
}
impl<R> Drop for TeamJob<R>{fn drop(&mut self){self.release_busy();}}
type TeamErrorHandler=Box<dyn FnOnce(&AppWindow,&AppContext,ApiError)>;
fn team_entry_error(app:&AppWindow,context:&AppContext,message:String){
    if let Ok(capture)=TeamCapture::new(context){capture.apply(context,||app.global::<AppState>().set_team_page_error(message.into()));}
}
fn reject_team_job_entry(app:&AppWindow,context:&AppContext,capture:&TeamCapture,page_busy:bool,current:&Rc<dyn Fn()->bool>,error:ApiError,on_error:TeamErrorHandler){
    let busy=team_busy_now(context);
    capture.apply(context,||if (current)(){
        if page_busy{app.global::<AppState>().set_team_page_loading(busy);}
        on_error(app,context,error);
    });
}
fn run_team_job<R:Send+'static>(
    app:&AppWindow,context:AppContext,capability:Option<KnownCapability>,
    work:impl FnOnce(TeamApi,SessionScope,Option<BillingScope>)->Result<R,ApiError>+Send+'static,
    apply:impl FnOnce(&AppWindow,&AppContext,R)+'static,
){
    let Ok(capture)=TeamCapture::new(&context)else{return;};
    let identity=(capture.persistence.lease().clone(),Uuid::new_v4());let expected=identity.clone();
    if capture.apply(&context,||TEAM_GROUP_REQUEST.with(|request|*request.borrow_mut()=Some(identity))).is_none(){return;}
    let current:Rc<dyn Fn()->bool>=Rc::new(move||TEAM_GROUP_REQUEST.with(|request|request.borrow().as_ref()==Some(&expected)));
    run_team_job_owned(app,context,capability,true,current,work,apply,
        Box::new(|app,_,error|app.global::<AppState>().set_team_page_error(error.user_message().into())));
}
fn run_team_job_handled<R:Send+'static>(
    app:&AppWindow,context:AppContext,capability:Option<KnownCapability>,page_busy:bool,
    work:impl FnOnce(TeamApi,SessionScope,Option<BillingScope>)->Result<R,ApiError>+Send+'static,
    apply:impl FnOnce(&AppWindow,&AppContext,R)+'static,on_error:TeamErrorHandler,
){
    run_team_job_owned(app,context,capability,page_busy,Rc::new(||true),work,apply,on_error);
}
fn run_team_job_owned<R:Send+'static>(
    app:&AppWindow,context:AppContext,capability:Option<KnownCapability>,page_busy:bool,current:Rc<dyn Fn()->bool>,
    work:impl FnOnce(TeamApi,SessionScope,Option<BillingScope>)->Result<R,ApiError>+Send+'static,
    apply:impl FnOnce(&AppWindow,&AppContext,R)+'static,on_error:TeamErrorHandler,
){
    let Ok(mut capture)=TeamCapture::new(&context)else{return;};
    let billing=match capability{
        Some(capability)=>match context.billing_context.current_scope(capability){
            Ok(billing) if billing.request.session==capture.scope=>Some(billing),
            Ok(_)=>{reject_team_job_entry(app,&context,&capture,page_busy,&current,ApiError::AuthenticationRequired,on_error);return;}
            Err(error)=>{reject_team_job_entry(app,&context,&capture,page_busy,&current,error,on_error);return;}
        },
        None=>None,
    };
    capture.billing=billing.clone();
    if !(current)() || !capture.current(&context){return;}
    let permit=match capture.persistence.begin_activity(){Ok(permit)=>permit,Err(_)=>return};
    let client=context.backend.as_ref().unwrap().api.clone();let scope=capture.scope.clone();
    let worker_scope=scope.clone();let worker_client=client.clone();let persistence=capture.persistence.clone();
    let cancel=Arc::new(std::sync::atomic::AtomicBool::new(false));let worker_cancel=cancel.clone();
    let id=Uuid::new_v4();let(sender,receiver)=mpsc::channel();
    #[cfg(test)]
    let after_send=TEAM_AFTER_SEND.with(|hook|hook.borrow_mut().take());
    let worker=std::thread::Builder::new().name("team-operation".into()).spawn(move||{
        let result=if worker_cancel.load(std::sync::atomic::Ordering::Acquire) || permit.is_quiescing()
            || !persistence.is_current() || !worker_client.user_work_is_current(&worker_scope){Err(ApiError::AuthenticationRequired)}
            else{work(TeamApi::new(client),scope,billing)};
        let result=match result{
            Ok(_) if worker_cancel.load(std::sync::atomic::Ordering::Acquire) || permit.is_quiescing()
                || !persistence.is_current() || !worker_client.user_work_is_current(&worker_scope)=>Err(ApiError::AuthenticationRequired),
            other=>other,
        };
        drop(permit);let _=sender.send(result);
        #[cfg(test)]
        if let Some(after_send)=after_send{after_send();}
    });
    let worker=match worker{
        Ok(worker)=>worker,
        Err(error)=>{reject_team_job_entry(app,&context,&capture,page_busy,&current,transition_error(error),on_error);return;}
    };
    TEAM_WORKERS.with(|workers|workers.borrow_mut().push(TeamWorker{id,lease:capture.persistence.lease().clone(),cancel,handle:worker}));
    let busy=if page_busy{
        let busy=TEAM_BUSY.with(Rc::clone);
        busy.borrow_mut().push(TeamBusy{id,capture:capture.clone(),current:current.clone()});
        capture.apply(&context,||if (current)(){let state=app.global::<AppState>();state.set_team_page_loading(true);state.set_team_page_error("".into());});
        Some(busy)
    }else{None};
    poll_team_job(app.as_weak(),context,capture,page_busy,Rc::new(RefCell::new(TeamJob{id,receiver,busy})),current,apply,on_error);
}
fn poll_team_job<R:Send+'static>(
    weak:Weak<AppWindow>,context:AppContext,capture:TeamCapture,page_busy:bool,job:Rc<RefCell<TeamJob<R>>>,
    current:Rc<dyn Fn()->bool>,apply:impl FnOnce(&AppWindow,&AppContext,R)+'static,on_error:TeamErrorHandler,
){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_team_workers();let id=job.borrow().id;
        let Some(app)=weak.upgrade()else{
            TEAM_WORKERS.with(|workers|for worker in workers.borrow().iter().filter(|worker|worker.id==id){worker.cancel.store(true,std::sync::atomic::Ordering::Release);});
            if team_worker_pending(id){poll_orphan_team_worker(id);}return;
        };
        if team_worker_pending(id){poll_team_job(weak,context,capture,page_busy,job,current,apply,on_error);return;}
        let result=match job.borrow().receiver.try_recv(){
            Ok(result)=>result,Err(TryRecvError::Disconnected)=>Err(transition_error("团队请求已中断")),
            Err(TryRecvError::Empty)=>{poll_team_job(weak,context,capture,page_busy,job.clone(),current,apply,on_error);return;}
        };
        job.borrow_mut().release_busy();
        if result.as_ref().err().is_some_and(|error|error.is_terminal_session_error())
            && capture.namespace_current(&context) && terminal_auth_scope_matches_context(&context,&capture.scope){
            // Real worker joined and counted activity dropped before control-plane dispatch.
            sign_out_locally(&app,&context,true,Some(capture.scope.auth_epoch));return;
        }
        let busy=team_busy_now(&context);
        capture.apply(&context,||{
            // UUID predicate is checked BEFORE rows, cursor, errors or shared busy projection.
            if !(current)(){if page_busy{app.global::<AppState>().set_team_page_loading(busy);}return;}
            if page_busy{app.global::<AppState>().set_team_page_loading(busy);}
            match result{Ok(value)=>apply(&app,&context,value),Err(error)=>on_error(&app,&context,error)}
        });
    });
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TeamPageKind { Members, Invitations, Pending, Usage }
impl TeamPageKind {
    fn capability(self) -> Option<KnownCapability> {
        match self {
            Self::Members => Some(KnownCapability::ManageMembers),
            Self::Invitations => Some(KnownCapability::ManageInvitations),
            Self::Pending => None,
            Self::Usage => Some(KnownCapability::ReadGroupUsage),
        }
    }
}
#[derive(Clone)]
struct TeamPagePosition {
    kind: TeamPageKind, session: SessionScope, billing: Option<BillingScope>,
    cursor: String, request: Uuid,
}
type TeamPages = Rc<RefCell<Vec<TeamPagePosition>>>;
enum TeamPageResult {
    Members(api::TeamPage<MemberView>),
    Invitations(api::TeamPage<OwnerInvitationView>),
    Pending(api::TeamPage<InvitationView>),
    Usage(api::TeamPage<UsageEventView>),
}
fn load_team_page(app: &AppWindow, context: AppContext, pages: TeamPages,
    kind: TeamPageKind, cursor: String, notice: Option<String>,
) {
    let Ok(capture)=TeamCapture::new(&context)else{return;};
    let session=capture.scope.clone();
    let billing = if kind.capability().is_some() { context.billing_context.confirmed_scope() } else { None };
    let request = Uuid::new_v4();
    let position = TeamPagePosition { kind, session, billing, cursor: cursor.clone(), request };
    // Keep only one bounded page per family; a newer page request wins over an older reply.
    if capture.apply(&context,||{pages.borrow_mut().retain(|old|old.kind!=kind);pages.borrow_mut().push(position);}).is_none(){return;}
    let request_pages=pages.clone();
    let current:Rc<dyn Fn()->bool>=Rc::new(move||request_pages.borrow().iter().any(|position|position.kind==kind && position.request==request));
    let error_pages = pages.clone();
    let error_notice = notice.clone();
    run_team_job_owned(app, context, kind.capability(), true,current, move |api, scope, billing| {
        let cursor = (!cursor.is_empty()).then_some(cursor.as_str());
        match kind {
            TeamPageKind::Members => api.list_members(&current_team_group(billing)?, cursor, &scope).map(TeamPageResult::Members),
            TeamPageKind::Invitations => api.list_invitations(&current_team_group(billing)?, cursor, &scope).map(TeamPageResult::Invitations),
            TeamPageKind::Pending => api.list_pending_invitations(cursor, &scope).map(TeamPageResult::Pending),
            TeamPageKind::Usage => api.usage_page(&current_team_group(billing)?, cursor, &scope).map(TeamPageResult::Usage),
        }
    }, move |app, _, page| {
        if !pages.borrow().iter().any(|position| position.kind == kind && position.request == request) { return; }
        let state = app.global::<AppState>();
        match page {
            TeamPageResult::Members(page) => {
                state.set_team_members(ModelRc::new(VecModel::from(page.items.into_iter().map(team_member_row).collect::<Vec<_>>())));
                state.set_team_members_next_cursor(page.next_cursor.unwrap_or_default().into());
            }
            TeamPageResult::Invitations(page) => {
                state.set_team_invitations(ModelRc::new(VecModel::from(page.items.into_iter().map(team_owner_invitation_row).collect::<Vec<_>>())));
                state.set_team_invitations_next_cursor(page.next_cursor.unwrap_or_default().into());
            }
            TeamPageResult::Pending(page) => {
                state.set_pending_team_invitations(ModelRc::new(VecModel::from(page.items.into_iter().map(team_invitation_row).collect::<Vec<_>>())));
                state.set_pending_team_invitations_next_cursor(page.next_cursor.unwrap_or_default().into());
            }
            TeamPageResult::Usage(page) => {
                state.set_team_usage(ModelRc::new(VecModel::from(page.items.into_iter().map(team_usage_row).collect::<Vec<_>>())));
                state.set_team_usage_next_cursor(page.next_cursor.unwrap_or_default().into());
            }
        }
        if let Some(notice) = notice { state.set_team_page_error(notice.into()); }
    }, Box::new(move |app, _, error| {
        if error_pages.borrow().iter().any(|position| position.kind == kind && position.request == request) {
            let message = error_notice.map(|notice| format!("{notice} 刷新失败，请重新加载。")).unwrap_or_else(|| error.user_message());
            app.global::<AppState>().set_team_page_error(message.into());
        }
    }));
}
#[derive(Clone, Copy)]
enum TeamRefreshTarget { Page(TeamPageKind), PendingAndGroups, SelectedGroup }
#[derive(Clone)]
struct TeamRefreshTicket {
    target: TeamRefreshTarget, session: SessionScope, lease: NamespaceLease,
    billing: Option<BillingScope>,persistence:PrivatePersistence,
}
impl TeamRefreshTicket {
    fn capture(context: &AppContext, _pages: &TeamPages, target: TeamRefreshTarget) -> Result<Self, ApiError> {
        let capture=TeamCapture::new(context)?;
        let session=capture.scope;let persistence=capture.persistence;
        let lease=persistence.lease().clone();
        let kind = match target {
            TeamRefreshTarget::Page(kind) => Some(kind), TeamRefreshTarget::PendingAndGroups => Some(TeamPageKind::Pending),
            TeamRefreshTarget::SelectedGroup => None,
        };
        let billing = if kind == Some(TeamPageKind::Pending) { None } else { context.billing_context.confirmed_scope() };
        Ok(Self { target, session, lease, billing,persistence })
    }
    fn current_cursor(&self,pages:&TeamPages,kind:TeamPageKind)->String{
        pages.borrow().iter().rev().find(|position|position.kind==kind
            && position.session==self.session && position.billing==self.billing)
            .map(|position|position.cursor.clone()).unwrap_or_default()
    }
    fn is_current(&self, context: &AppContext) -> bool {
        self.persistence.is_current() && context.store.borrow().private_persistence.as_ref().is_some_and(|value|value.lease()==self.persistence.lease())
            && context.backend.as_ref().is_some_and(|backend| backend.api.user_work_is_current(&self.session))
            && context.namespace_for(&self.session).ok().as_ref() == Some(&self.lease)
            && self.billing.as_ref().is_none_or(|scope| context.billing_context.is_current(scope))
    }
}
fn team_conflict_requires_refresh(error: &ApiError) -> bool {
    matches!(error, ApiError::Protocol { .. }) || matches!(error.code(), Some("account_group_version_conflict" | "membership_version_conflict"
        | "invitation_version_conflict" | "invitation_expired" | "invitation_revoked"
        | "invitation_superseded" | "invitation_consumed" | "invitation_declined"))
}
fn schedule_captured_team_refresh(app: &AppWindow, context: AppContext, pages: TeamPages,
    ticket: TeamRefreshTicket, notice: Option<String>,
) {
    let weak = app.as_weak();
    // Never nest admission/latch acquisition inside apply_user_completion.
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(app) = weak.upgrade() else { return; };
        if !ticket.is_current(&context) { return; }
        match ticket.target {
            TeamRefreshTarget::Page(kind) => {
                let cursor=ticket.current_cursor(&pages,kind);
                load_team_page(&app, context, pages, kind, cursor, notice);
            },
            TeamRefreshTarget::PendingAndGroups => {
                // Membership might have changed, but this identity-only read never changes selection.
                refresh_team_groups(&app, context.clone());
                let cursor=ticket.current_cursor(&pages,TeamPageKind::Pending);
                load_team_page(&app, context, pages, TeamPageKind::Pending, cursor, notice);
            }
            TeamRefreshTarget::SelectedGroup => refresh_selected_team_metadata(&app, context, ticket, notice),
        }
    });
}
fn run_team_mutation<R: Send + 'static>(
    app: &AppWindow, context: AppContext, pages: TeamPages, capability: Option<KnownCapability>,
    target: TeamRefreshTarget,
    work: impl FnOnce(TeamApi, SessionScope, Option<BillingScope>) -> Result<R, ApiError> + Send + 'static,
) {
    let ticket = match TeamRefreshTicket::capture(&context, &pages, target) {
        Ok(ticket) => ticket, Err(error) => { team_entry_error(app,&context,error.user_message()); return; }
    };
    let success_ticket = ticket.clone();
    let success_pages = pages.clone();
    let form_token = app.global::<AppState>().get_team_form_token();
    let success_token = form_token.clone();
    let action = app.global::<AppState>().get_team_form_action();
    app.global::<AppState>().set_team_page_status("".into());
    run_team_job_handled(app, context, capability, true, work,
        move |app, context, _| {
            let state = app.global::<AppState>();
            if success_token.is_empty() || (state.get_team_panel_visible() && state.get_team_form_token() == success_token) {
                presentation::clear_form(&state);
                state.set_team_page_status(presentation::success_label(&action).into());
            }
            schedule_captured_team_refresh(app, context.clone(), success_pages, success_ticket, None);
        },
        Box::new(move |app, context, error| {
            let refresh = team_conflict_requires_refresh(&error);
            let notice = if refresh { "状态已变化，请核对刷新后的信息，再确认操作。".to_owned() } else { error.user_message() };
            let state = app.global::<AppState>();
            let visible = form_token.is_empty() || (state.get_team_panel_visible() && state.get_team_form_token() == form_token);
            if visible {
                state.set_team_page_error(notice.clone().into());
                if refresh && !form_token.is_empty() { state.set_team_form_needs_review(true); }
            }
            if refresh { schedule_captured_team_refresh(app, context.clone(), pages, ticket, visible.then_some(notice)); }
        }));
}
fn refresh_selected_team_metadata(app: &AppWindow, context: AppContext, ticket: TeamRefreshTicket, notice: Option<String>) {
    let requested = ticket.billing.clone();
    run_team_job(app, context, None, |api, scope, _| api.list_groups(&scope), move |app, context, groups| {
        // The current-read result still belongs to the group captured by the original mutation.
        if requested.as_ref().is_none_or(|scope| !context.billing_context.is_current(scope)) { return; }
        app.global::<AppState>().set_pending_team_invitation_count(groups.pending_invitation_count.to_string().into());
        let candidate = groups.items.iter().find(|choice| requested.as_ref()
            .is_some_and(|scope| choice.group_id == scope.request.account_group_id)).cloned();
        let state = app.global::<AppState>();
        let preserve_rename = notice.is_some() && state.get_team_panel_visible()
            && state.get_team_form_action() == "rename" && state.get_team_form_needs_review()
            && candidate.as_ref().is_some_and(|choice| context.billing_context.confirmed_snapshot()
                .is_some_and(|snapshot| forms::same_billing_authority(choice, &snapshot)));
        context.team_groups.replace(groups.items);
        render_team_context(app, context);
        if let Some(notice) = notice { app.global::<AppState>().set_team_page_error(notice.into()); }
        if preserve_rename {
            // This read changes display metadata and optimistic version only. Keep
            // the current durable billing authority and require explicit review.
            if let Some(choice) = candidate { state.set_team_current_name(choice.name.into()); }
            return;
        }
        let weak = app.as_weak(); let context = context.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(app) = weak.upgrade() else { return; };
            if !ticket.is_current(&context) { return; }
            match candidate {
                Some(choice) if choice.selectable && choice.readable_context => {
                    // Reconfirm name/version through the durable coordinator. No raw snapshot setter.
                    if let Some(coordinator) = context.account_transition.clone() {
                        coordinator.switch_billing(&app, context, choice, PreviousBillingAuthority::StillValid);
                    } else {
                        team_entry_error(&app,&context,"账号确认尚未就绪，请稍后重新加载。".into());
                    }
                }
                _ => {
                    context.billing_context.invalidate_current_billing();
                    schedule_team_fallback(&app, context);
                }
            }
        });
    });
}

pub(super) fn refresh_team_groups(app: &AppWindow, context: AppContext) {
    run_team_job(app, context, None, |api, scope, _| api.list_groups(&scope), |app, context, groups| {
        app.global::<AppState>().set_pending_team_invitation_count(groups.pending_invitation_count.to_string().into());
        let current = context.billing_context.confirmed_scope();
        let confirmed = context.billing_context.confirmed_snapshot();
        let invalid = current.as_ref().is_some_and(|scope| !groups.items.iter().any(|choice|
            choice.group_id == scope.request.account_group_id && choice.readable_context
                && confirmed.as_ref().is_some_and(|snapshot| choice.selectable == snapshot.billing_group.selectable
                    && choice.capabilities == snapshot.capabilities
                    && choice.relationship_status == snapshot.billing_group.relationship_status)));
        context.team_groups.replace(groups.items);
        if invalid { context.billing_context.invalidate_current_billing(); }
        render_team_context(app, context);
        if invalid { schedule_team_fallback(app, context.clone()); }
        else if app.global::<AppState>().get_team_panel_visible()
            && app.global::<AppState>().get_team_tab() == "accounts"
            && app.global::<AppState>().get_team_can_manage_members() {
            let weak = app.as_weak(); let context = context.clone();
            let expected = context.billing_context.confirmed_scope();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(app) = weak.upgrade() {
                    let state = app.global::<AppState>();
                    if state.get_team_panel_visible() && state.get_team_tab() == "accounts"
                        && expected.as_ref().is_some_and(|scope| context.billing_context.is_current(scope)) {
                        state.invoke_load_team_members("".into());
                    }
                }
            });
        }
    });
}
fn schedule_team_fallback(app: &AppWindow, context: AppContext) {
    let Ok(capture)=TeamCapture::snapshot(&context)else{return;};
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(app) = weak.upgrade() else { return; };
        if !capture.current(&context){return;}
        let candidate = BillingContextManager::choose_candidate(&context.team_groups.borrow(), None, None).ok().cloned();
        if let (Some(candidate), Some(coordinator)) = (candidate, context.account_transition.clone()) {
            coordinator.switch_billing(&app, context, candidate, PreviousBillingAuthority::Invalidated);
        } else {
            capture.apply(&context,||{
                context.billing_context.invalidate_for_auth_change();
                clear_account_snapshot_state(&app, &context); render_team_context(&app, &context);
            });
        }
    });
}
fn schedule_team_refresh(app: &AppWindow, context: AppContext) {
    let Ok(capture)=TeamCapture::snapshot(&context)else{return;};
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || { if let Some(app) = weak.upgrade() {if capture.current(&context){refresh_team_groups(&app, context);}} });
}
fn current_team_group(billing: Option<BillingScope>) -> Result<String, ApiError> {
    billing.map(|scope| scope.request.account_group_id).ok_or(ApiError::AuthenticationRequired)
}

#[derive(Default)]
struct TeamReauthForm {
    key: Option<(SessionScope, NamespaceLease, String)>,
    token: Option<Uuid>,
    code_busy: bool,
    confirm_busy: bool,
    resend_deadline: Option<Instant>,
}
type TeamReauth = Rc<RefCell<TeamReauthForm>>;

fn clear_team_reauth_presentation(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_team_reauth_password("".into()); state.set_team_reauth_code("".into());
    state.set_team_reauth_status("".into()); state.set_team_reauth_error("".into());
    state.set_team_reauth_email_mask("".into()); state.set_team_reauth_countdown(0);
    state.set_team_reauth_busy(false); state.set_team_reauth_code_busy(false);
}
fn sync_team_reauth_form(app:&AppWindow,context:&AppContext,form:&TeamReauth){
    let state=app.global::<AppState>();
    let capture=TeamCapture::new(context);
    if !state.get_team_reauth_visible() || capture.is_err(){
        // Control-plane disposal is allowed after retirement/426; it never publishes a proof.
        let mut value=form.borrow_mut();value.key=None;value.token=None;
        value.code_busy=false;value.confirm_busy=false;value.resend_deadline=None;drop(value);
        clear_team_reauth_presentation(app);return;
    }
    let capture=capture.unwrap();
    let mut mode=state.get_team_reauth_mode().to_string();
    let available=|mode:&str|match mode{"current_password"=>state.get_password_set(),"email_code"=>state.get_email_bound(),_=>false};
    if !available(&mode){mode=if state.get_password_set(){"current_password"}else if state.get_email_bound(){"email_code"}else{""}.into();}
    let key=available(&mode).then(||(capture.scope.clone(),capture.persistence.lease().clone(),mode.clone()));
    capture.apply(context,||{
        if state.get_team_reauth_mode().as_str()!=mode{state.set_team_reauth_mode(mode.into());}
        if form.borrow().key!=key{
            let mut value=form.borrow_mut();value.key=key.clone();
            value.token=value.key.as_ref().map(|_|Uuid::new_v4());value.code_busy=false;value.confirm_busy=false;value.resend_deadline=None;drop(value);
            clear_team_reauth_presentation(app);
        }else if key.is_none(){clear_team_reauth_presentation(app);}
        let remaining=form.borrow().resend_deadline.map(|deadline|deadline.saturating_duration_since(Instant::now()).as_secs_f64().ceil() as u64).unwrap_or(0);
        state.set_team_reauth_countdown(remaining.min(i32::MAX as u64) as i32);
    });
}
fn watch_team_reauth(weak: Weak<AppWindow>, context: AppContext, form: TeamReauth) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        reap_team_workers();if TEAM_SHUTDOWN.with(|closed|closed.get()){return;}
        let Some(app) = weak.upgrade() else { return; };
        // User/namespace replacement and 426 can happen without a Slint visibility change.
        sync_team_reauth_form(&app, &context, &form);
        watch_team_reauth(weak, context, form);
    });
}
fn team_reauth_token_is_current(app: &AppWindow, form: &TeamReauth, token: Uuid) -> bool {
    let state = app.global::<AppState>();
    let value = form.borrow();
    value.token == Some(token) && state.get_team_reauth_visible()
        && value.key.as_ref().is_some_and(|(_, _, mode)| mode == state.get_team_reauth_mode().as_str())
}
fn request_team_reauth_code(app: &AppWindow, context: AppContext, form: TeamReauth) {
    sync_team_reauth_form(app, &context, &form);
    let Ok(capture)=TeamCapture::new(&context)else{return;};
    let state = app.global::<AppState>();
    let token = capture.apply(&context,||{
        let mut value = form.borrow_mut();
        let Some(token) = value.token else { return None; };
        if value.code_busy || value.confirm_busy || !state.get_email_bound()
            || state.get_team_reauth_mode() != "email_code"
            || value.resend_deadline.is_some_and(|deadline| deadline > Instant::now()) { return None; }
        value.code_busy = true;
        state.set_team_reauth_code_busy(true); state.set_team_reauth_error("".into());
        state.set_team_reauth_status("".into()); state.set_team_reauth_code("".into());Some(token)
    }).flatten();
    let Some(token)=token else{return;};
    let error_form = form.clone();
    run_team_job_handled(app, context, None, false,
        |api, scope, _| api.request_reauthentication_code(&scope),
        move |app, _, code| {
            if !team_reauth_token_is_current(app, &form, token) { return; }
            let state = app.global::<AppState>();
            let mut value = form.borrow_mut(); value.code_busy = false;
            // Reject unrepresentable server durations instead of silently shortening its cooldown.
            let deadline = Instant::now().checked_add(Duration::from_secs(code.resend_after_seconds));
            if code.resend_after_seconds > i32::MAX as u64 || deadline.is_none() {
                state.set_team_reauth_code_busy(false);
                state.set_team_reauth_error("验证邮件响应无效，请稍后重试。".into());
                return;
            }
            value.resend_deadline = deadline;
            state.set_team_reauth_code_busy(false);
            state.set_team_reauth_countdown(code.resend_after_seconds as i32);
            state.set_team_reauth_email_mask(code.email_masked.clone().into());
            state.set_team_reauth_status(format!("验证邮件已发送至 {}", code.email_masked).into());
        }, Box::new(move |app, _, error| {
            if !team_reauth_token_is_current(app, &error_form, token) { return; }
            error_form.borrow_mut().code_busy = false;
            app.global::<AppState>().set_team_reauth_code_busy(false);
            app.global::<AppState>().set_team_reauth_error(error.user_message().into());
        }));
}
fn confirm_team_reauth(app: &AppWindow, context: AppContext, form: TeamReauth) {
    sync_team_reauth_form(app, &context, &form);
    let Ok(capture)=TeamCapture::new(&context)else{return;};
    let state = app.global::<AppState>();
    let prepared=capture.apply(&context,||{
    let (token, mode) = {
        let value = form.borrow();
        let Some(token) = value.token else { return None; };
        if value.confirm_busy || value.code_busy { return None; }
        (token, state.get_team_reauth_mode().to_string())
    };
    // Read exactly the selected method; hidden residual input cannot select a different proof.
    let secret = SecretString::new(match mode.as_str() {
        "current_password" if state.get_password_set() => state.get_team_reauth_password().to_string(),
        "email_code" if state.get_email_bound() => state.get_team_reauth_code().trim().to_owned(),
        _ => return None,
    });
    state.set_team_reauth_password("".into()); state.set_team_reauth_code("".into());
    if secret.expose().is_empty() {
        state.set_team_reauth_error("请输入当前密码或邮箱验证码。".into()); return None;
    }
    form.borrow_mut().confirm_busy = true;
    state.set_team_reauth_busy(true); state.set_team_reauth_error("".into()); state.set_team_reauth_status("".into());
    Some((token,mode,secret))
    }).flatten();
    let Some((token,mode,secret))=prepared else{return;};
    let error_form = form.clone();
    run_team_job_handled(app, context, None, false, move |api, scope, _| {
        let request = if mode == "current_password" { ReauthenticationRequest::current_password(secret.expose()) }
            else { ReauthenticationRequest::email_code(secret.expose()) };
        let proof = api.reauthenticate(request, &scope)?;
        if proof.user_id != scope.owner_user_id { return Err(transition_error("重新验证身份不一致")); }
        Ok(proof)
    }, move |app, _, proof| {
        if !team_reauth_token_is_current(app, &form, token) { return; }
        form.borrow_mut().confirm_busy = false;
        let state = app.global::<AppState>(); state.set_team_reauth_busy(false);
        state.set_team_reauth_password("".into()); state.set_team_reauth_code("".into());
        state.set_team_reauth_status(format!("身份已验证，有效期至 {}", proof.expires_at).into());
    }, Box::new(move |app, _, error| {
        if !team_reauth_token_is_current(app, &error_form, token) { return; }
        error_form.borrow_mut().confirm_busy = false;
        app.global::<AppState>().set_team_reauth_busy(false);
        app.global::<AppState>().set_team_reauth_error(error.user_message().into());
    }));
}


pub(super) fn wire_team_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let team_forms = forms::wire(app, context.clone());
    let pages: TeamPages = Rc::new(RefCell::new(Vec::new()));
    let form: TeamReauth = Rc::new(RefCell::new(TeamReauthForm::default()));
    { let weak = app.as_weak(); let context = context.clone(); let form = form.clone();
      state.on_team_reauth_lifecycle_changed(move || {
        if let Some(app)=weak.upgrade(){
            if !app.global::<AppState>().get_team_reauth_visible(){
                let mut value=form.borrow_mut();value.key=None;value.token=None;value.code_busy=false;value.confirm_busy=false;value.resend_deadline=None;drop(value);
                clear_team_reauth_presentation(&app);
            }else{
                // Property changes may originate in another short completion. Defer admission.
                let weak=weak.clone();let context=context.clone();let form=form.clone();
                slint::Timer::single_shot(Duration::ZERO,move||if let Some(app)=weak.upgrade(){sync_team_reauth_form(&app,&context,&form);});
            }
        }
      }); }
    sync_team_reauth_form(app, &context, &form);
    watch_team_reauth(app.as_weak(), context.clone(), form.clone());
    { let weak = app.as_weak(); let context = context.clone();
      state.on_refresh_account_groups(move || { if let Some(app) = weak.upgrade() { refresh_team_groups(&app, context.clone()); } }); }
    { let weak = app.as_weak(); let context = context.clone();
      state.on_switch_account_group(move |id| {
        let Some(app) = weak.upgrade() else { return; };
        let Ok(capture)=TeamCapture::new(&context)else{return;};
        if !capture.current(&context){return;}
        let choice = context.team_groups.borrow().iter().find(|choice| choice.group_id == id.as_str() && choice.selectable).cloned();
        if let (Some(choice), Some(coordinator)) = (choice, context.account_transition.clone()) {
            coordinator.switch_billing(&app, context.clone(), choice, PreviousBillingAuthority::StillValid);
        }
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_load_team_members(move |cursor| { if let Some(app) = weak.upgrade() {
        load_team_page(&app, context.clone(), pages.clone(), TeamPageKind::Members, cursor.to_string(), None);
      } }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_load_team_invitations(move |cursor| { if let Some(app) = weak.upgrade() {
        load_team_page(&app, context.clone(), pages.clone(), TeamPageKind::Invitations, cursor.to_string(), None);
      } }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_load_pending_team_invitations(move |cursor| { if let Some(app) = weak.upgrade() {
        load_team_page(&app, context.clone(), pages.clone(), TeamPageKind::Pending, cursor.to_string(), None);
      } }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_load_team_usage(move |cursor| { if let Some(app) = weak.upgrade() {
        load_team_page(&app, context.clone(), pages.clone(), TeamPageKind::Usage, cursor.to_string(), None);
      } }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_rename_team(move || {
        let Some(app) = weak.upgrade() else { return; };
        let name = match team_name(&app.global::<AppState>().get_team_name_input()) {
            Ok(value) => value, Err(error) => { team_entry_error(&app,&context,error.user_message()); return; }
        };
        let Some(snapshot) = context.billing_context.confirmed_snapshot() else { return; };
        let version = if app.global::<AppState>().get_team_form_action() == "rename" {
            let Some(version) = team_forms.rename_version(&app, &context) else { return; };
            version
        } else { snapshot.billing_group.group_version };
        let key = Uuid::new_v4().to_string();
        run_team_mutation(&app, context.clone(), pages.clone(), Some(KnownCapability::ManageGroup), TeamRefreshTarget::SelectedGroup,
            move |api, scope, billing| api.rename_group(&current_team_group(billing)?, &name, &version, &key, &scope));
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_create_team_invitation(move || {
        let Some(app) = weak.upgrade() else { return; }; let state = app.global::<AppState>();
        let email = state.get_team_invite_email().trim().to_ascii_lowercase();
        if !valid_email(&email) { team_entry_error(&app,&context,"请输入正确的邮箱地址".into()); return; }
        let limit = match team_decimal(&state.get_team_invite_limit_input()) {
            Ok(value) => value, Err(error) => { team_entry_error(&app,&context,error.user_message()); return; }
        };
        let key = Uuid::new_v4().to_string();
        run_team_mutation(&app, context.clone(), pages.clone(), Some(KnownCapability::ManageInvitations), TeamRefreshTarget::Page(TeamPageKind::Invitations),
            move |api, scope, billing| api.create_invitation(&current_team_group(billing)?, &email, &limit, &key, &scope));
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_resend_team_invitation(move |id, version| {
        let Some(app) = weak.upgrade() else { return; };
        let limit = match team_decimal(&app.global::<AppState>().get_team_limit_input()) {
            Ok(value) => value, Err(error) => { team_entry_error(&app,&context,error.user_message()); return; }
        };
        let (id, version, key) = (id.to_string(), version.to_string(), Uuid::new_v4().to_string());
        run_team_mutation(&app, context.clone(), pages.clone(), Some(KnownCapability::ManageInvitations), TeamRefreshTarget::Page(TeamPageKind::Invitations),
            move |api, scope, billing| {
                let invitation = api.resend_invitation(&current_team_group(billing)?, &id, &limit, &version, &key, &scope)?;
                if invitation.invitation_id != id {
                    return Err(ApiError::Protocol { message: "重新发送邀请的响应身份不一致".into(), request_id: None });
                }
                Ok(invitation)
            });
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_revoke_team_invitation(move |id, version| {
        let Some(app) = weak.upgrade() else { return; };
        let (id, version, key) = (id.to_string(), version.to_string(), Uuid::new_v4().to_string());
        run_team_mutation(&app, context.clone(), pages.clone(), Some(KnownCapability::ManageInvitations), TeamRefreshTarget::Page(TeamPageKind::Invitations),
            move |api, scope, billing| api.revoke_invitation(&current_team_group(billing)?, &id, &version, &key, &scope));
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_accept_team_invitation(move |id, version| {
        let Some(app) = weak.upgrade() else { return; };
        let (id, version, key) = (id.to_string(), version.to_string(), Uuid::new_v4().to_string());
        run_team_mutation(&app, context.clone(), pages.clone(), None, TeamRefreshTarget::PendingAndGroups,
            move |api, scope, _| api.accept_invitation(&id, &version, &key, &scope));
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_decline_team_invitation(move |id, version| {
        let Some(app) = weak.upgrade() else { return; };
        let (id, version, key) = (id.to_string(), version.to_string(), Uuid::new_v4().to_string());
        run_team_mutation(&app, context.clone(), pages.clone(), None, TeamRefreshTarget::Page(TeamPageKind::Pending),
            move |api, scope, _| api.decline_invitation(&id, &version, &key, &scope));
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_update_team_member(move |id, version, action| {
        let Some(app) = weak.upgrade() else { return; };
        let limit = if action == "set_limit" {
            match team_decimal(&app.global::<AppState>().get_team_limit_input()) {
                Ok(value) => value, Err(error) => { team_entry_error(&app,&context,error.user_message()); return; }
            }
        } else { String::new() };
        let (id, version, action, key) = (id.to_string(), version.to_string(), action.to_string(), Uuid::new_v4().to_string());
        run_team_mutation(&app, context.clone(), pages.clone(), Some(KnownCapability::ManageMembers), TeamRefreshTarget::Page(TeamPageKind::Members),
            move |api, scope, billing| {
                let group = current_team_group(billing)?;
                match action.as_str() {
                    "remove" => api.remove_member(&group, &id, &version, &key, &scope),
                    "suspend" => api.update_member(&group, &id, MemberPolicyAction::Suspend, &version, &key, &scope),
                    "resume" => api.update_member(&group, &id, MemberPolicyAction::Resume, &version, &key, &scope),
                    "set_limit" => api.update_member(&group, &id, MemberPolicyAction::SetLimit { monthly_credit_limit: &limit }, &version, &key, &scope),
                    _ => Err(transition_error("不支持的成员操作")),
                }
            });
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pages = pages.clone();
      state.on_leave_team(move || {
        let Some(app) = weak.upgrade() else { return; };
        let Some(snapshot) = context.billing_context.confirmed_snapshot() else { return; };
        let (Some(id), Some(version)) = (snapshot.billing_group.member_id, snapshot.billing_group.membership_version) else { return; };
        let key = Uuid::new_v4().to_string();
        run_team_mutation(&app, context.clone(), pages.clone(), Some(KnownCapability::LeaveTeam), TeamRefreshTarget::SelectedGroup,
            move |api, scope, billing| api.leave_group(&current_team_group(billing)?, &id, &version, &key, &scope));
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let form = form.clone();
      state.on_request_team_reauth_code(move || {
        if let Some(app) = weak.upgrade() { request_team_reauth_code(&app, context.clone(), form.clone()); }
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let form = form.clone();
      state.on_confirm_team_reauth(move || {
        if let Some(app) = weak.upgrade() { confirm_team_reauth(&app, context.clone(), form.clone()); }
      }); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn core_team_inputs_preserve_i64_decimals_and_unicode_names() {
        assert_eq!(team_decimal("0").unwrap(), "0");
        assert_eq!(team_decimal("9223372036854775807").unwrap(), "9223372036854775807");
        for invalid in ["", "-1", "不限", "1.0", "1e3", "9223372036854775808"] { assert!(team_decimal(invalid).is_err()); }
        assert_eq!(team_name("  创作团队  ").unwrap(), "创作团队");
        assert!(team_name(&"创".repeat(129)).is_err());
        assert!(team_name(" ").is_err());
    }
}
