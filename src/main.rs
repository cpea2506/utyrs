mod capabilities;
mod document_storage;
mod notification;
mod request;

use crate::{
    notification::{NotificationHandle, UnityNotification},
    request::{RequestHandle, UnityRequest},
};
use document_storage::DocumentStorage;
use gen_lsp_types::{
    CodeLensOptions, InitializeParams, PositionEncodingKind, ServerCapabilities, TextDocumentSync,
    TextDocumentSyncKind, TextDocumentSyncOptions, WorkspaceFolders::WorkspaceFolderList,
    WorkspaceFoldersServerCapabilities, WorkspaceOptions,
};
use lsp_server::{Connection, Message};
use std::{error::Error, result::Result};
use tracing::{error, info};

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    info!("Starting Unity LS");

    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = serde_json::to_value(ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF8),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(true),
            ..Default::default()
        }),
        workspace: Some(WorkspaceOptions {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        text_document_sync: Some(TextDocumentSync::from(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::Full),
            ..Default::default()
        })),
        ..Default::default()
    })?;

    let params = connection.initialize(server_capabilities)?;

    main_loop(connection, params)?;
    io_threads.join()?;

    info!("Unity LS stopped");

    Ok(())
}

fn main_loop(
    connection: Connection,
    params: serde_json::Value,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let params = serde_json::from_value::<InitializeParams>(params)?;
    let mut docs = DocumentStorage::new();

    if let Some(WorkspaceFolderList(workspace_folders)) =
        params.workspace_folders_initialize_params.workspace_folders
    {
        let workspace_root = &workspace_folders[0].uri;

        for msg in &connection.receiver {
            match msg {
                Message::Request(request) => {
                    if connection.handle_shutdown(&request)? {
                        break;
                    }

                    let unity_request =
                        UnityRequest::new(&connection, &request, &docs, workspace_root);

                    if let Err(err) = unity_request.handle() {
                        error!("[Unity LS] Request {} failed: {err}", &request.method);
                    }
                }
                Message::Notification(notification) => {
                    let mut unity_notification = UnityNotification::new(&notification, &mut docs);

                    if let Err(err) = unity_notification.handle() {
                        error!(
                            "[Unity LS] Notification {} failed: {err}",
                            notification.method
                        );
                    }
                }
                Message::Response(response) => {
                    info!("Received response: {:?}", response);
                }
            }
        }
    }

    Ok(())
}
