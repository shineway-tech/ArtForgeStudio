use super::*;

#[derive(Debug, thiserror::Error)]
pub(super) enum DeliveryRetryError {
    #[error("authentication is required")]
    AuthenticationRequired,
    #[error("the generated file has expired")]
    Expired,
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Local(#[from] anyhow::Error),
}

impl DeliveryDownloadKey {
    fn new(session_scope: &SessionScope, client_request_id: &str, file_id: &str) -> Self {
        Self {
            owner_user_id: session_scope.owner_user_id.clone(),
            auth_epoch: session_scope.auth_epoch,
            client_request_id: client_request_id.to_string(),
            file_id: file_id.to_string(),
        }
    }

    fn belongs_to_scope(&self, session_scope: &SessionScope) -> bool {
        self.owner_user_id == session_scope.owner_user_id
            && self.auth_epoch == session_scope.auth_epoch
    }
}

pub(super) fn try_reserve_delivery_download_pairs(
    registry: &GenerationRegistry,
    session_scope: &SessionScope,
    pairs: &[(String, String)],
) -> Option<Vec<DeliveryDownloadReservation>> {
    let keys = pairs
        .iter()
        .map(|(client_request_id, file_id)| {
            DeliveryDownloadKey::new(session_scope, client_request_id, file_id)
        })
        .collect::<BTreeSet<_>>();
    let mut downloads = registry.delivery_downloads.borrow_mut();
    if keys.iter().any(|key| downloads.contains_key(key)) {
        return None;
    }
    if keys.is_empty() {
        return Some(Vec::new());
    }
    let mut reservation_id = registry
        .next_delivery_download_reservation_id
        .get()
        .wrapping_add(1);
    if reservation_id == 0 {
        reservation_id = 1;
    }
    registry
        .next_delivery_download_reservation_id
        .set(reservation_id);
    let reservations = keys
        .into_iter()
        .map(|key| DeliveryDownloadReservation {
            key,
            reservation_id,
        })
        .collect::<Vec<_>>();
    downloads.extend(
        reservations
            .iter()
            .map(|reservation| (reservation.key.clone(), reservation.reservation_id)),
    );
    Some(reservations)
}

pub(super) fn release_delivery_download_reservations_in_registry(
    registry: &GenerationRegistry,
    reservations: &[DeliveryDownloadReservation],
) -> bool {
    let mut downloads = registry.delivery_downloads.borrow_mut();
    let mut released = false;
    for reservation in reservations {
        if downloads.get(&reservation.key) == Some(&reservation.reservation_id) {
            downloads.remove(&reservation.key);
            released = true;
        }
    }
    released
}

pub(super) fn release_delivery_download_reservations(
    context: &AppContext,
    reservations: &[DeliveryDownloadReservation],
) -> bool {
    release_delivery_download_reservations_in_registry(&context.generations, reservations)
}

pub(super) fn complete_delivery_download(
    app: &AppWindow,
    context: &AppContext,
    reservation: &DeliveryDownloadReservation,
) {
    release_delivery_download_reservations(context, std::slice::from_ref(reservation));
    refresh_delivery_download_flags(app, context);
}

pub(super) fn release_delivery_downloads_for_scope(
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    let mut downloads = context.generations.delivery_downloads.borrow_mut();
    let previous_len = downloads.len();
    downloads.retain(|key, _| !key.belongs_to_scope(session_scope));
    downloads.len() != previous_len
}

pub(super) fn refresh_delivery_download_flags(app: &AppWindow, context: &AppContext) {
    let Some(scope) = current_generation_session_scope(context) else {
        let mut store = context.store.borrow_mut();
        for asset in &mut store.generations {
            asset.delivery_downloading = false;
        }
        push_generations(app, &store);
        return;
    };
    let downloads = context.generations.delivery_downloads.borrow();
    let mut store = context.store.borrow_mut();
    for asset in &mut store.generations {
        if !asset.delivery_recoverable {
            asset.delivery_downloading = false;
            continue;
        }
        asset.delivery_downloading = recoverable_delivery_for_failed_asset(
            &scope.owner_user_id,
            scope.auth_epoch,
            &asset.id,
        )
        .ok()
        .flatten()
        .is_some_and(|(record, delivery)| {
            downloads.contains_key(&DeliveryDownloadKey::new(
                &scope,
                &record.client_request_id,
                &delivery.file_id,
            ))
        });
    }
    drop(downloads);
    push_generations(app, &store);
}

fn set_failed_delivery_downloading(
    app: &AppWindow,
    context: &AppContext,
    failed_asset_id: &str,
    downloading: bool,
) {
    let mut store = context.store.borrow_mut();
    if let Some(asset) = store
        .generations
        .iter_mut()
        .find(|asset| asset.id == failed_asset_id && asset.source_path == "failed")
    {
        asset.delivery_downloading = downloading;
    }
    push_generations(app, &store);
}

fn clear_failed_delivery_recovery(app: &AppWindow, context: &AppContext, failed_asset_id: &str) {
    let mut store = context.store.borrow_mut();
    if let Some(asset) = store
        .generations
        .iter_mut()
        .find(|asset| asset.id == failed_asset_id && asset.source_path == "failed")
    {
        asset.delivery_recoverable = false;
        asset.delivery_downloading = false;
    }
    push_generations(app, &store);
}

pub(super) fn reserve_recovered_delivery_download_pairs(
    registry: &GenerationRegistry,
    record: &PendingGenerationRecord,
) -> Option<Vec<DeliveryDownloadReservation>> {
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    let pairs = record
        .deliveries
        .iter()
        .filter(|delivery| {
            !delivery.failed_asset_id.trim().is_empty()
                && delivery.local_path.trim().is_empty()
                && !delivery.acknowledged
                && !delivery.abandoned
        })
        .map(|delivery| (record.client_request_id.clone(), delivery.file_id.clone()))
        .collect::<Vec<_>>();
    try_reserve_delivery_download_pairs(registry, &session_scope, &pairs)
}

pub(super) fn reserve_recovered_delivery_downloads(
    app: &AppWindow,
    context: &AppContext,
    record: &PendingGenerationRecord,
) -> Option<Vec<DeliveryDownloadReservation>> {
    let pending = record
        .deliveries
        .iter()
        .filter(|delivery| {
            !delivery.failed_asset_id.trim().is_empty()
                && delivery.local_path.trim().is_empty()
                && !delivery.acknowledged
                && !delivery.abandoned
        })
        .collect::<Vec<_>>();
    let reservations = reserve_recovered_delivery_download_pairs(&context.generations, record)?;
    if !pending.is_empty() {
        let failed_ids = pending
            .iter()
            .map(|delivery| delivery.failed_asset_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut store = context.store.borrow_mut();
        for asset in &mut store.generations {
            if failed_ids.contains(asset.id.as_str()) && asset.source_path == "failed" {
                asset.delivery_downloading = true;
            }
        }
        push_generations(app, &store);
    }
    Some(reservations)
}

pub(super) fn select_recoverable_task_file<'a>(
    detail: &'a GenerationTaskDetail,
    delivery: &PendingDeliveryRecord,
) -> std::result::Result<&'a TaskOutputFile, DeliveryRetryError> {
    let file = detail
        .items
        .iter()
        .find(|item| item.index == delivery.item_index && item.status == "succeeded")
        .and_then(|item| item.file.as_ref())
        .filter(|file| file.id == delivery.file_id)
        .filter(|file| file.status == "available")
        .filter(|file| {
            file.download_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty())
        })
        .ok_or(DeliveryRetryError::Expired)?;
    let size_bytes = file
        .size_bytes
        .parse::<u64>()
        .map_err(|_| {
            DeliveryRetryError::Api(ApiError::Protocol {
                message: "服务端返回了无效的文件大小".to_string(),
                request_id: None,
            })
        })?;
    if size_bytes != delivery.size_bytes || !file.sha256.eq_ignore_ascii_case(&delivery.sha256) {
        return Err(DeliveryRetryError::Api(ApiError::Protocol {
            message: "生成文件完整性信息与恢复记录不一致".to_string(),
            request_id: None,
        }));
    }
    Ok(file)
}

fn retry_api_error(error: ApiError) -> DeliveryRetryError {
    if matches!(&error, ApiError::Http { status: 404, .. })
        || matches!(
            error.code(),
            Some("generation_task_not_found" | "result_file_not_found" | "result_file_expired")
        )
    {
        DeliveryRetryError::Expired
    } else if matches!(error, ApiError::AuthenticationRequired) {
        DeliveryRetryError::AuthenticationRequired
    } else {
        DeliveryRetryError::Api(error)
    }
}

pub(super) fn pending_delivery_saved_then_acknowledge_with<P, A>(
    persist_saved_delivery: P,
    acknowledge: A,
) -> Result<bool>
where
    P: FnOnce() -> Result<bool>,
    A: FnOnce(),
{
    let saved = persist_saved_delivery()?;
    if saved {
        acknowledge();
    }
    Ok(saved)
}

enum DeliveryCompletionError {
    Local(anyhow::Error),
    Recovery,
}

fn local_save_then_record_and_acknowledge_with<L, P, A>(
    save_local: L,
    persist_saved_delivery: P,
    acknowledge: A,
) -> std::result::Result<(String, bool), DeliveryCompletionError>
where
    L: FnOnce() -> Result<String>,
    P: FnOnce(&str) -> Result<bool>,
    A: FnOnce(),
{
    let source_path = save_local().map_err(DeliveryCompletionError::Local)?;
    let saved = pending_delivery_saved_then_acknowledge_with(
        || persist_saved_delivery(&source_path),
        acknowledge,
    )
    .map_err(|_| DeliveryCompletionError::Recovery)?;
    Ok((source_path, saved))
}

pub(super) struct RetrySuccess {
    pub(super) staged_path: PathBuf,
    pub(super) delivery: DeliveryConfirmation,
}

pub(super) fn run_failed_delivery_retry(
    api: &GenerationApi,
    scope: &SessionScope,
    record: &PendingGenerationRecord,
    delivery: &PendingDeliveryRecord,
) -> std::result::Result<RetrySuccess, DeliveryRetryError> {
    if record.owner_user_id != scope.owner_user_id
        || record.auth_epoch != scope.auth_epoch
        || record.server_task_id.trim().is_empty()
        || delivery.file_id.trim().is_empty()
    {
        return Err(DeliveryRetryError::AuthenticationRequired);
    }
    let detail = api
        .task_scoped(&record.server_task_id, scope)
        .map_err(retry_api_error)?;
    let file = select_recoverable_task_file(&detail, delivery)?;
    let staging_path =
        generation_download_staging_path(&record.client_request_id, delivery.item_index, file);
    cleanup_failed_delivery_staging(&staging_path);
    if let Err(error) = api.download_verified_to_path_scoped(file, scope, &staging_path) {
        cleanup_failed_delivery_staging(&staging_path);
        return Err(retry_api_error(error));
    }
    if let Err(error) = inspect_image_dimensions(&staging_path) {
        cleanup_failed_delivery_staging(&staging_path);
        return Err(DeliveryRetryError::Local(error));
    }
    Ok(RetrySuccess {
        staged_path: staging_path,
        delivery: DeliveryConfirmation {
            client_request_id: record.client_request_id.clone(),
            item_index: delivery.item_index,
            task_id: detail.id,
            file_id: delivery.file_id.clone(),
            sha256: delivery.sha256.clone(),
            size_bytes: delivery.size_bytes,
            failed_asset_id: Some(delivery.failed_asset_id.clone()),
        },
    })
}

pub(super) fn retry_failed_delivery(app: &AppWindow, context: AppContext, failed_asset_id: String) {
    let state = app.global::<AppState>();
    let Some(backend) = context.backend.clone() else {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let Some(scope) = current_generation_session_scope(&context) else {
        state.set_generation_status("请先登录后再重新下载".into());
        return;
    };
    let (record, delivery) = match recoverable_delivery_for_failed_asset(
        &scope.owner_user_id,
        scope.auth_epoch,
        &failed_asset_id,
    ) {
        Ok(Some(value)) => value,
        Ok(None) => {
            clear_failed_delivery_recovery(app, &context, &failed_asset_id);
            state.set_generation_status("文件已过期，请重新生成".into());
            return;
        }
        Err(_) => {
            state.set_generation_status("本地生成恢复记录无法读取，请重启后重试".into());
            return;
        }
    };
    let pair = (record.client_request_id.clone(), delivery.file_id.clone());
    let Some(mut reservations) = try_reserve_delivery_download_pairs(
        &context.generations,
        &scope,
        std::slice::from_ref(&pair),
    ) else {
        return;
    };
    let reservation = reservations
        .pop()
        .expect("a non-empty delivery pair reserves one key");
    set_failed_delivery_downloading(app, &context, &failed_asset_id, true);
    state.set_generation_status("正在重新下载图片...".into());

    let worker_scope = scope.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let result = run_failed_delivery_retry(&api, &worker_scope, &record, &delivery);
        let _ = sender.send(result);
    });
    poll_failed_delivery_retry(
        app.as_weak(),
        context,
        scope,
        failed_asset_id,
        reservation,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_failed_delivery_retry(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    scope: SessionScope,
    failed_asset_id: String,
    reservation: DeliveryDownloadReservation,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<RetrySuccess, DeliveryRetryError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = {
            let mut receiver = receiver.borrow_mut();
            let Some(channel) = receiver.as_ref() else {
                drop(receiver);
                if let Some(app) = app_weak.upgrade() {
                    complete_delivery_download(&app, &context, &reservation);
                } else {
                    release_delivery_download_reservations(
                        &context,
                        std::slice::from_ref(&reservation),
                    );
                }
                return;
            };
            match channel.try_recv() {
                Ok(result) => {
                    receiver.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    receiver.take();
                    Some(Err(DeliveryRetryError::Local(anyhow!(
                        "delivery retry worker disconnected"
                    ))))
                }
            }
        };
        let Some(result) = result else {
            poll_failed_delivery_retry(
                app_weak,
                context,
                scope,
                failed_asset_id,
                reservation,
                receiver,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            release_delivery_download_reservations(
                &context,
                std::slice::from_ref(&reservation),
            );
            return;
        };
        if !generation_scope_matches_context(&context, &scope) {
            complete_delivery_download(&app, &context, &reservation);
            set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
            return;
        }
        match result {
            Ok(success) => {
                let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
                let delivery_for_persistence = success.delivery.clone();
                let completion = local_save_then_record_and_acknowledge_with(
                    || {
                        replace_failed_delivery_asset_checked(
                            &app,
                            &context.store,
                            &failed_asset_id,
                            &success.staged_path,
                            &time,
                        )
                        .map(|(source_path, _)| source_path)
                    },
                    |source_path| {
                        pending_delivery_saved(
                            &scope.owner_user_id,
                            scope.auth_epoch,
                            &delivery_for_persistence.client_request_id,
                            &delivery_for_persistence,
                            source_path,
                        )
                    },
                    || {
                        acknowledge_delivery_after_local_save(
                            app.as_weak(),
                            context.clone(),
                            scope.clone(),
                            success.delivery,
                        );
                    },
                );
                match completion {
                    Ok((_, true)) => app
                        .global::<AppState>()
                        .set_generation_status("图片下载完成".into()),
                    Ok((_, false)) | Err(DeliveryCompletionError::Recovery) => {
                        app.global::<AppState>().set_generation_status(
                            "图片已保存，但恢复记录更新失败；稍后将继续清理远端文件".into(),
                        );
                    }
                    Err(DeliveryCompletionError::Local(error)) => {
                        cleanup_failed_delivery_staging(&success.staged_path);
                        set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                        app.global::<AppState>().set_generation_status(
                            format!("图片下载失败：{}", zh_error(&error.to_string())).into(),
                        );
                    }
                }
            }
            Err(DeliveryRetryError::Expired) => {
                if matches!(
                    abandon_pending_delivery(
                        &scope.owner_user_id,
                        scope.auth_epoch,
                        &failed_asset_id,
                    ),
                    Ok(true)
                ) {
                    clear_failed_delivery_recovery(&app, &context, &failed_asset_id);
                    app.global::<AppState>()
                        .set_generation_status("文件已过期，请重新生成".into());
                } else {
                    set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                    app.global::<AppState>()
                        .set_generation_status("本地生成恢复记录无法更新，请重启后重试".into());
                }
            }
            Err(DeliveryRetryError::AuthenticationRequired) => {
                set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                app.global::<AppState>()
                    .set_generation_status("登录状态已变化，请重新登录后下载".into());
            }
            Err(DeliveryRetryError::Api(error)) => {
                set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                app.global::<AppState>().set_generation_status(
                    format!("图片下载失败：{}", error.generation_message()).into(),
                );
            }
            Err(DeliveryRetryError::Local(error)) => {
                set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                app.global::<AppState>().set_generation_status(
                    format!("图片下载失败：{}", zh_error(&error.to_string())).into(),
                );
            }
        }
        complete_delivery_download(&app, &context, &reservation);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_file(id: &str, status: &str) -> TaskOutputFile {
        TaskOutputFile {
            id: id.to_string(),
            status: status.to_string(),
            mime_type: "image/png".to_string(),
            size_bytes: "3".to_string(),
            sha256: "abc".to_string(),
            width: Some(1),
            height: Some(1),
            download_url: Some(format!("https://example.invalid/{id}.png")),
        }
    }

    fn completed_task_with_two_files() -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "task-1".to_string(),
            status: "completed".to_string(),
            progress_percent: 100,
            success_count: 2,
            failure_count: 0,
            failure: None,
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 2,
            task_type: "image_generation".to_string(),
            items: vec![
                GenerationTaskItem {
                    index: 0,
                    status: "succeeded".to_string(),
                    credit_cost: "1".to_string(),
                    failure: None,
                    file: Some(task_file("file-1", "available")),
                },
                GenerationTaskItem {
                    index: 1,
                    status: "succeeded".to_string(),
                    credit_cost: "1".to_string(),
                    failure: None,
                    file: Some(task_file("file-2", "available")),
                },
            ],
        }
    }

    fn delivery(item_index: usize, file_id: &str) -> PendingDeliveryRecord {
        PendingDeliveryRecord {
            item_index,
            file_id: file_id.to_string(),
            sha256: "abc".to_string(),
            size_bytes: 3,
            failed_asset_id: "failed-1".to_string(),
            ..PendingDeliveryRecord::default()
        }
    }

    fn scope(owner_user_id: &str, auth_epoch: u64) -> SessionScope {
        SessionScope {
            owner_user_id: owner_user_id.to_string(),
            auth_epoch,
        }
    }

    fn recoverable_record(scope: &SessionScope) -> PendingGenerationRecord {
        PendingGenerationRecord {
            schema_version: 1,
            created_at_epoch_ms: 0,
            client_request_id: "request-a".to_string(),
            owner_user_id: scope.owner_user_id.clone(),
            auth_epoch: scope.auth_epoch,
            local_task_id: "local-task".to_string(),
            server_task_id: "server-task".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            task_type: "image_generation".to_string(),
            category: "character".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "model".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: true,
            reference_paths: Vec::new(),
            reference_sha256: Vec::new(),
            reference_size_bytes: Vec::new(),
            lineage_reference_paths: Vec::new(),
            uploaded_file_ids: Vec::new(),
            deliveries: vec![delivery(0, "file-1")],
            terminal: true,
            expected_success_count: 1,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    #[test]
    fn retry_selects_only_the_original_successful_file() {
        let detail = completed_task_with_two_files();
        let selected = select_recoverable_task_file(&detail, &delivery(1, "file-2")).unwrap();

        assert_eq!(selected.id, "file-2");
    }

    #[test]
    fn retry_rejects_a_different_file_at_the_original_item_index() {
        let detail = completed_task_with_two_files();

        assert!(matches!(
            select_recoverable_task_file(&detail, &delivery(1, "file-1")),
            Err(DeliveryRetryError::Expired)
        ));
    }

    #[test]
    fn retry_rejects_an_expired_or_deleted_original_file() {
        for status in ["expired", "deleted"] {
            let mut detail = completed_task_with_two_files();
            detail.items[1].file.as_mut().unwrap().status = status.to_string();

            assert!(matches!(
                select_recoverable_task_file(&detail, &delivery(1, "file-2")),
                Err(DeliveryRetryError::Expired)
            ));
        }
    }

    #[test]
    fn available_file_with_invalid_size_remains_recoverable() {
        let mut detail = completed_task_with_two_files();
        detail.items[1].file.as_mut().unwrap().size_bytes = "invalid".to_string();

        assert!(matches!(
            select_recoverable_task_file(&detail, &delivery(1, "file-2")),
            Err(DeliveryRetryError::Api(ApiError::Protocol { .. }))
        ));
    }

    #[test]
    fn available_file_with_integrity_mismatch_remains_recoverable() {
        let mut mismatched_size = completed_task_with_two_files();
        mismatched_size.items[1].file.as_mut().unwrap().size_bytes = "4".to_string();
        let mut mismatched_sha = completed_task_with_two_files();
        mismatched_sha.items[1].file.as_mut().unwrap().sha256 = "def".to_string();

        for detail in [&mismatched_size, &mismatched_sha] {
            assert!(matches!(
                select_recoverable_task_file(detail, &delivery(1, "file-2")),
                Err(DeliveryRetryError::Api(ApiError::Protocol { .. }))
            ));
        }
    }

    #[test]
    fn delivery_download_key_captures_scope_request_and_file_identity() {
        let scope_a = scope("user-a", 7);
        let scope_b = scope("user-b", 7);
        let newer_scope_a = scope("user-a", 8);
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&scope_a, "request-b", "file-1")
        );
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-2")
        );
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&scope_b, "request-a", "file-1")
        );
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&newer_scope_a, "request-a", "file-1")
        );
    }

    #[test]
    fn delivery_download_pair_reservation_is_atomic_when_one_pair_is_in_flight() {
        let registry = GenerationRegistry::default();
        let scope = scope("user-a", 7);
        let occupied = [("request-a".to_string(), "file-1".to_string())];
        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &occupied).is_some());
        let pairs = vec![
            ("request-a".to_string(), "file-2".to_string()),
            ("request-a".to_string(), "file-1".to_string()),
        ];

        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &pairs).is_none());
        assert!(!registry.delivery_downloads.borrow().contains_key(
            &DeliveryDownloadKey::new(&scope, "request-a", "file-2")
        ));
    }

    #[test]
    fn stopping_an_automatically_recovered_task_releases_only_its_exact_reservations() {
        let context = AppContext::default();
        let scope = scope("user-a", 7);
        let category = "character".to_string();
        let task_pair = [("request-a".to_string(), "file-1".to_string())];
        let unrelated_pair = [("request-a".to_string(), "file-2".to_string())];
        let reservations = try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &task_pair,
        )
        .expect("reserve recovered task delivery");
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &unrelated_pair,
        )
        .is_some());
        insert_active_generation(
            &context,
            ActiveGeneration {
                task_id: "recovered-task".to_string(),
                client_request_id: Some(task_pair[0].0.clone()),
                category: category.clone(),
                session_scope: scope.clone(),
                delivery_download_reservations: reservations,
                ..ActiveGeneration::default()
            },
        );

        assert!(remove_active_generation(&context, &category, "recovered-task").is_some());

        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &task_pair,
        )
        .is_some());
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &unrelated_pair,
        )
        .is_none());
    }

    #[test]
    fn generation_account_teardown_clears_only_the_captured_scope_reservations() {
        let context = AppContext::default();
        let scope_a = scope("user-a", 7);
        let scope_b = scope("user-b", 7);
        let pair = [("request-a".to_string(), "file-1".to_string())];
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_a,
            &pair,
        )
        .is_some());
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_b,
            &pair,
        )
        .is_some());

        release_delivery_downloads_for_scope(&context, &scope_a);

        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_a,
            &pair,
        )
        .is_some());
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_b,
            &pair,
        )
        .is_none());
    }

    #[test]
    fn manual_and_automatic_recovery_mutually_exclude_the_same_scoped_pair() {
        let registry = GenerationRegistry::default();
        let scope = scope("user-a", 7);
        let record = recoverable_record(&scope);
        let pair = [(record.client_request_id.clone(), "file-1".to_string())];

        let automatic = reserve_recovered_delivery_download_pairs(&registry, &record)
            .expect("automatic recovery reserves pair");
        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &pair).is_none());

        release_delivery_download_reservations_in_registry(&registry, &automatic);
        let manual = try_reserve_delivery_download_pairs(&registry, &scope, &pair)
            .expect("manual retry reserves released pair");
        assert!(reserve_recovered_delivery_download_pairs(&registry, &record).is_none());

        release_delivery_download_reservations_in_registry(&registry, &manual);
    }

    #[test]
    fn stale_reservation_cleanup_cannot_release_a_new_reservation_for_the_same_key() {
        let registry = GenerationRegistry::default();
        let scope = scope("user-a", 7);
        let pair = [("request-a".to_string(), "file-1".to_string())];
        let stale = try_reserve_delivery_download_pairs(&registry, &scope, &pair)
            .expect("reserve original worker");
        release_delivery_download_reservations_in_registry(&registry, &stale);
        let current = try_reserve_delivery_download_pairs(&registry, &scope, &pair)
            .expect("reserve replacement worker");

        release_delivery_download_reservations_in_registry(&registry, &stale);

        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &pair).is_none());
        release_delivery_download_reservations_in_registry(&registry, &current);
    }

    #[test]
    fn only_not_found_or_expired_api_errors_abandon_manual_recovery() {
        let not_found = ApiError::Http {
            status: 404,
            code: "generation_task_not_found".to_string(),
            message: "missing".to_string(),
            request_id: None,
            details: None,
        };
        let transient = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };

        assert!(matches!(
            retry_api_error(not_found),
            DeliveryRetryError::Expired
        ));
        assert!(matches!(
            retry_api_error(transient),
            DeliveryRetryError::Api(_)
        ));
    }

    #[test]
    fn acknowledgement_runs_only_after_the_saved_delivery_record_is_durable() {
        let events = RefCell::new(Vec::new());

        let saved = pending_delivery_saved_then_acknowledge_with(
            || {
                events.borrow_mut().push("pending_delivery_saved");
                Ok(true)
            },
            || events.borrow_mut().push("acknowledge"),
        )
        .unwrap();

        assert!(saved);
        assert_eq!(
            events.into_inner(),
            vec!["pending_delivery_saved", "acknowledge"]
        );
    }

    #[test]
    fn persistence_error_never_invokes_the_acknowledgement_hook() {
        let acknowledged = std::cell::Cell::new(false);

        let result = pending_delivery_saved_then_acknowledge_with(
            || Err(anyhow!("recovery file is not durable")),
            || acknowledged.set(true),
        );

        assert!(result.is_err());
        assert!(!acknowledged.get());
    }

    #[test]
    fn local_persistence_error_stops_before_recovery_record_and_acknowledgement() {
        let events = RefCell::new(Vec::new());

        let result = local_save_then_record_and_acknowledge_with(
            || {
                events.borrow_mut().push("local_persistence");
                Err(anyhow!("disk full"))
            },
            |_| {
                events.borrow_mut().push("pending_delivery_saved");
                Ok(true)
            },
            || events.borrow_mut().push("acknowledge"),
        );

        assert!(matches!(result, Err(DeliveryCompletionError::Local(_))));
        assert_eq!(events.into_inner(), vec!["local_persistence"]);
    }
}
