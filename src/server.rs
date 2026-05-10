use crate::{capabilities::codelens::UnityCodeLens, document_storage::DocumentStorage};
use gen_lsp_types::{
    CodeLens, CodeLensOptions, CodeLensParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams, LspNotificationMethod,
    LspRequestMethod, PositionEncodingKind, RootPath, ServerCapabilities,
    TextDocumentContentChangeEvent, TextDocumentSync, TextDocumentSyncKind,
    TextDocumentSyncOptions, WorkspaceFolder, WorkspaceFolders, WorkspaceFoldersServerCapabilities,
    WorkspaceOptions,
};
use lsp_server::{
    Connection, ErrorCode, Message, Notification, Request, RequestId, Response, ResponseError,
};
use std::{error::Error, io, path::PathBuf};
use tracing::{error, info};

pub fn run_stdio() -> Result<(), Box<dyn Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    let initialize_params = connection.initialize(server_capabilities()?)?;
    let mut server = UnityServer::new(connection, initialize_params)?;

    server.run()?;
    io_threads.join()?;

    Ok(())
}

struct UnityServer {
    connection: Connection,
    docs: DocumentStorage,
    workspace_root: PathBuf,
}

impl UnityServer {
    fn new(
        connection: Connection,
        initialize_params: serde_json::Value,
    ) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let params = serde_json::from_value::<InitializeParams>(initialize_params)?;
        let workspace_root = workspace_root(&params)?;

        Ok(Self {
            connection,
            docs: DocumentStorage::new(),
            workspace_root,
        })
    }

    fn run(&mut self) -> Result<(), Box<dyn Error + Sync + Send>> {
        while let Ok(msg) = self.connection.receiver.recv() {
            match msg {
                Message::Request(request) => {
                    if self.connection.handle_shutdown(&request)? {
                        break;
                    }

                    if let Err(err) = self.handle_request(&request) {
                        error!("[Unity LS] Request {} failed: {err}", request.method);
                    }
                }
                Message::Notification(notification) => {
                    if let Err(err) = self.handle_notification(&notification) {
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

        Ok(())
    }

    fn handle_request(&self, request: &Request) -> Result<(), Box<dyn Error + Sync + Send>> {
        match LspRequestMethod::from(request.method.clone()) {
            LspRequestMethod::TextDocumentCodeLens => {
                let params = serde_json::from_value::<CodeLensParams>(request.params.clone())?;
                let uri = params.text_document.uri;
                let codelenses = self
                    .docs
                    .get(&uri)
                    .map(|content| UnityCodeLens::create(&self.workspace_root, content, &uri))
                    .transpose()?
                    .unwrap_or_default();

                send_ok(&self.connection, request.id.clone(), &codelenses)?;
            }
            LspRequestMethod::CodeLensResolve => {
                let codelens = serde_json::from_value::<CodeLens>(request.params.clone())?;
                let resolved = UnityCodeLens::resolve(codelens)?;

                send_ok(&self.connection, request.id.clone(), &resolved)?;
            }
            _ => {
                send_err(
                    &self.connection,
                    request.id.clone(),
                    ErrorCode::MethodNotFound,
                    &format!("unhandled method: {}", request.method),
                )?;
            }
        }

        Ok(())
    }

    fn handle_notification(
        &mut self,
        notification: &Notification,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        match LspNotificationMethod::from(notification.method.clone()) {
            LspNotificationMethod::TextDocumentDidOpen => {
                let params = serde_json::from_value::<DidOpenTextDocumentParams>(
                    notification.params.clone(),
                )?;
                self.docs
                    .open(params.text_document.uri, params.text_document.text);
            }
            LspNotificationMethod::TextDocumentDidChange => {
                let params = serde_json::from_value::<DidChangeTextDocumentParams>(
                    notification.params.clone(),
                )?;
                if let Some(
                    TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(change),
                ) = params.content_changes.into_iter().next()
                {
                    self.docs.change(
                        params.text_document.text_document_identifier.uri,
                        change.text,
                    );
                }
            }
            LspNotificationMethod::TextDocumentDidClose => {
                let params = serde_json::from_value::<DidCloseTextDocumentParams>(
                    notification.params.clone(),
                )?;
                self.docs.close(&params.text_document.uri);
            }
            _ => {}
        }

        Ok(())
    }
}

#[allow(deprecated)]
fn workspace_root(params: &InitializeParams) -> Result<PathBuf, Box<dyn Error + Sync + Send>> {
    if let Some(workspace_folder) =
        workspace_folder_list(params).and_then(|folders| folders.first())
    {
        return workspace_folder
            .uri
            .to_file_path()
            .map_err(|_| io::Error::other("workspace folder URI must be a file URI"))
            .map_err(Into::into);
    }

    if let Some(root_uri) = &params.root_uri {
        return root_uri
            .to_file_path()
            .map_err(|_| io::Error::other("rootUri must be a file URI"))
            .map_err(Into::into);
    }

    if let Some(RootPath::String(root_path)) = &params.root_path {
        return Ok(PathBuf::from(root_path));
    }

    Err(io::Error::other("workspace root required").into())
}

#[allow(deprecated)]
fn workspace_folder_list(params: &InitializeParams) -> Option<&Vec<WorkspaceFolder>> {
    match params
        .workspace_folders_initialize_params
        .workspace_folders
        .as_ref()?
    {
        WorkspaceFolders::WorkspaceFolderList(workspace_folders)
            if !workspace_folders.is_empty() =>
        {
            Some(workspace_folders)
        }
        WorkspaceFolders::WorkspaceFolderList(_) | WorkspaceFolders::Null => None,
    }
}

fn server_capabilities() -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(ServerCapabilities {
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
    })
}

fn send_ok<T: serde::Serialize>(
    connection: &Connection,
    id: RequestId,
    payload: &T,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let response = Response {
        id,
        result: Some(serde_json::to_value(payload)?),
        error: None,
    };
    connection.sender.send(Message::Response(response))?;
    Ok(())
}

fn send_err(
    connection: &Connection,
    id: RequestId,
    code: ErrorCode,
    message: &str,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let response = Response {
        id,
        result: None,
        error: Some(ResponseError {
            code: code as i32,
            message: message.to_string(),
            data: None,
        }),
    };
    connection.sender.send(Message::Response(response))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gen_lsp_types::{
        ClientCapabilities, Uri, WorkDoneProgressParams, WorkspaceFolder,
        WorkspaceFoldersInitializeParams,
    };

    fn initialize_params(
        root_uri: Option<Uri>,
        root_path: Option<RootPath>,
        workspace_folders: Option<Vec<WorkspaceFolder>>,
    ) -> InitializeParams {
        InitializeParams::new(
            None,
            None,
            None,
            root_path,
            root_uri,
            ClientCapabilities::default(),
            None,
            None,
            WorkDoneProgressParams::new(None),
            WorkspaceFoldersInitializeParams::new(
                workspace_folders.map(WorkspaceFolders::WorkspaceFolderList),
            ),
        )
    }

    #[test]
    fn uses_workspace_folder_before_other_roots() {
        let params = initialize_params(
            Some(Uri::from_file_path("/tmp/root-uri").expect("file uri")),
            Some(RootPath::from("/tmp/root-path")),
            Some(vec![WorkspaceFolder::new(
                Uri::from_file_path("/tmp/workspace").expect("file uri"),
                "workspace".to_string(),
            )]),
        );

        let root = workspace_root(&params).expect("workspace root");

        assert_eq!(root, PathBuf::from("/tmp/workspace"));
    }

    #[test]
    fn falls_back_to_root_uri() {
        let params = initialize_params(
            Some(Uri::from_file_path("/tmp/root-uri").expect("file uri")),
            Some(RootPath::from("/tmp/root-path")),
            None,
        );

        let root = workspace_root(&params).expect("workspace root");

        assert_eq!(root, PathBuf::from("/tmp/root-uri"));
    }

    #[test]
    fn falls_back_to_root_path() {
        let params = initialize_params(None, Some(RootPath::from("/tmp/root-path")), None);

        let root = workspace_root(&params).expect("workspace root");

        assert_eq!(root, PathBuf::from("/tmp/root-path"));
    }
}
