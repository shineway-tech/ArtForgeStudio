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

pub(super) fn delivery_download_key(client_request_id: &str, file_id: &str) -> String {
    format!(
        "{}:{}:{}",
        client_request_id.len(),
        client_request_id,
        file_id
    )
}

fn delivery_download_key_has_request(key: &str, client_request_id: &str) -> bool {
    key.starts_with(&format!(
        "{}:{}:",
        client_request_id.len(),
        client_request_id
    ))
}

fn try_reserve_delivery_download_pairs(
    downloads: &RefCell<BTreeSet<String>>,
    pairs: &[(String, String)],
) -> Option<BTreeSet<String>> {
    let keys = pairs
        .iter()
        .map(|(client_request_id, file_id)| delivery_download_key(client_request_id, file_id))
        .collect::<BTreeSet<_>>();
    let mut downloads = downloads.borrow_mut();
    if keys.iter().any(|key| downloads.contains(key)) {
        return None;
    }
    downloads.extend(keys.iter().cloned());
    Some(keys)
}

fn release_delivery_download_key(context: &AppContext, client_request_id: &str, file_id: &str) {
    context
        .generations
        .delivery_downloads
        .borrow_mut()
        .remove(&delivery_download_key(client_request_id, file_id));
}

pub(super) fn complete_delivery_download(
    app: &AppWindow,
    context: &AppContext,
    client_request_id: &str,
    file_id: &str,
) {
    release_delivery_download_key(context, client_request_id, file_id);
    refresh_delivery_download_flags(app, context);
}

pub(super) fn release_delivery_downloads_for_request(
    app: &AppWindow,
    context: &AppContext,
    client_request_id: &str,
) {
    context
        .generations
        .delivery_downloads
        .borrow_mut()
        .retain(|key| !delivery_download_key_has_request(key, client_request_id));
    refresh_delivery_download_flags(app, context);
}

fn refresh_delivery_download_flags(app: &AppWindow, context: &AppContext) {
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
            downloads.contains(&delivery_download_key(
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

pub(super) fn reserve_recovered_delivery_downloads(
    app: &AppWindow,
    context: &AppContext,
    record: &PendingGenerationRecord,
) -> bool {
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
    let pairs = pending
        .iter()
        .map(|delivery| (record.client_request_id.clone(), delivery.file_id.clone()))
        .collect::<Vec<_>>();
    if try_reserve_delivery_download_pairs(&context.generations.delivery_downloads, &pairs)
        .is_none()
    {
        return false;
    }
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
    true
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
    if try_reserve_delivery_download_pairs(
        &context.generations.delivery_downloads,
        std::slice::from_ref(&pair),
    )
    .is_none()
    {
        return;
    }
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
        pair.0,
        pair.1,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_failed_delivery_retry(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    scope: SessionScope,
    failed_asset_id: String,
    client_request_id: String,
    file_id: String,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<RetrySuccess, DeliveryRetryError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = {
            let mut receiver = receiver.borrow_mut();
            let Some(channel) = receiver.as_ref() else {
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
                client_request_id,
                file_id,
                receiver,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            release_delivery_download_key(&context, &client_request_id, &file_id);
            return;
        };
        if !generation_scope_matches_context(&context, &scope) {
            release_delivery_download_key(&context, &client_request_id, &file_id);
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
        complete_delivery_download(&app, &context, &client_request_id, &file_id);
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
    fn delivery_download_key_distinguishes_the_exact_request_and_file_pair() {
        assert_ne!(
            delivery_download_key("request-a", "file-1"),
            delivery_download_key("request-b", "file-1")
        );
        assert_ne!(
            delivery_download_key("request-a", "file-1"),
            delivery_download_key("request-a", "file-2")
        );
    }

    #[test]
    fn delivery_download_pair_reservation_is_atomic_when_one_pair_is_in_flight() {
        let downloads = RefCell::new(BTreeSet::from([delivery_download_key(
            "request-a",
            "file-1",
        )]));
        let pairs = vec![
            ("request-a".to_string(), "file-2".to_string()),
            ("request-a".to_string(), "file-1".to_string()),
        ];

        assert!(try_reserve_delivery_download_pairs(&downloads, &pairs).is_none());
        assert!(!downloads
            .borrow()
            .contains(&delivery_download_key("request-a", "file-2")));
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
