use gen_lsp_types::{CodeLens, Command, Location, Position, Range, Uri};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{self, Display},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use tree_sitter::{Node, Parser};
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: [&str; 3] = ["unity", "prefab", "asset"];

/// Result type for code lens operations.
pub type CodeLensResult<T> = Result<T, CodeLensError>;

/// Error type for code lens operations.
#[derive(Debug)]
pub enum CodeLensError {
    /// Failed to serialize data to JSON.
    SerializationFailed(String),
    /// Failed to deserialize data from JSON.
    DeserializationFailed(String),
    /// Invalid file URI.
    InvalidUri(Uri),
    /// Invalid file path.
    InvalidPath(PathBuf),
    /// Missing required code lens data.
    MissingData,
    /// Unable to read metadata file.
    MetadataError(String),
}

impl Display for CodeLensError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SerializationFailed(msg) => write!(f, "serialization failed: {}", msg),
            Self::DeserializationFailed(msg) => write!(f, "deserialization failed: {}", msg),
            Self::InvalidUri(uri) => {
                write!(f, "URI must be in the file scheme or similar: {}", uri)
            }
            Self::InvalidPath(path) => {
                write!(
                    f,
                    "path must be absolute and must exist: {}",
                    path.display()
                )
            }
            Self::MissingData => write!(f, "missing code lens data"),
            Self::MetadataError(msg) => write!(f, "metadata error: {}", msg),
        }
    }
}

impl Error for CodeLensError {}

/// A reference to a script in a Unity asset file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetReference {
    /// Asset file path.
    pub path: PathBuf,
    /// Line number of the reference.
    pub line_number: u32,
}

impl AssetReference {
    /// Converts to an LSP Location for CodeLens.
    ///
    /// # Errors
    ///
    /// Returns `CodeLensError::InvalidPath` if the path cannot be converted to a URI.
    pub fn to_codelens_location(&self) -> CodeLensResult<Location> {
        let uri = Uri::from_file_path(&self.path)
            .map_err(|_| CodeLensError::InvalidPath(self.path.clone()))?;

        Ok(Location {
            uri,
            range: Range {
                start: Position::new(self.line_number, 0),
                end: Position::new(self.line_number, 1),
            },
        })
    }
}

/// Unity code lens (two-stage LSP resolution).
#[derive(Debug)]
pub struct UnityCodeLens;

impl UnityCodeLens {
    /// Creates an unresolved code lens with embedded asset reference data.
    ///
    /// # Errors
    ///
    /// Returns `CodeLensError` if serialization fails.
    pub fn create(workspace_root: &Uri, content: &str, uri: Uri) -> CodeLensResult<Vec<CodeLens>> {
        let assets_dir = workspace_root
            .to_file_path()
            .map_err(|_| CodeLensError::InvalidUri(uri.clone()))?
            .join("Assets");

        let class_line = parse_class_line(content).unwrap_or(0);
        let asset_references = find_references(&assets_dir, uri)?;

        let data = serde_json::to_value(asset_references)
            .map_err(|e| CodeLensError::SerializationFailed(e.to_string()))?;

        Ok(vec![CodeLens {
            range: Range {
                start: Position::new(class_line, 0),
                end: Position::new(class_line, 1),
            },
            command: None,
            data: Some(data),
        }])
    }

    /// Resolves an unresolved code lens to a fully resolved lens with command.
    ///
    /// # Errors
    ///
    /// Returns `CodeLensError` if deserialization or location building fails.
    pub fn resolve(mut codelens: CodeLens) -> CodeLensResult<CodeLens> {
        let data = codelens.data.take().ok_or(CodeLensError::MissingData)?;
        let asset_references = serde_json::from_value::<Vec<AssetReference>>(data)
            .map_err(|e| CodeLensError::DeserializationFailed(e.to_string()))?;

        let locations = asset_references
            .iter()
            .filter_map(|r| r.to_codelens_location().ok())
            .collect::<Vec<Location>>();
        let count = locations.len();
        let title = match count {
            0 | 1 => format!("{} Unity reference", count),
            _ => format!("{} Unity references", count),
        };
        let arguments = serde_json::to_value(locations)
            .map_err(|e| CodeLensError::SerializationFailed(e.to_string()))?;

        Ok(CodeLens {
            range: codelens.range,
            command: Some(Command {
                title,
                command: "showUnityReferences".to_string(),
                arguments: Some(vec![arguments]),
                ..Default::default()
            }),
            data: None,
        })
    }
}

fn find_references(assets_dir: &Path, script_uri: Uri) -> CodeLensResult<Vec<AssetReference>> {
    let meta_path = script_uri
        .to_file_path()
        .map_err(|_| CodeLensError::InvalidUri(script_uri.clone()))?
        .with_extension("cs.meta");

    let script_guid = extract_guid_from_meta(&meta_path)?;

    Ok(find_asset_references(assets_dir, &script_guid))
}

fn extract_guid_from_meta(meta_path: &Path) -> CodeLensResult<String> {
    /* First two lines of Unity meta file:
     *
     * fileFormatVersion: 2
     * guid: 83c14770bb100154e969c8bc1a4f153c
     * */

    let file =
        File::open(meta_path).map_err(|_| CodeLensError::InvalidPath(meta_path.to_path_buf()))?;

    BufReader::new(file)
        .lines()
        .find_map(|l| l.ok()?.strip_prefix("guid: ").map(str::to_owned))
        .ok_or_else(|| CodeLensError::MetadataError("guid not found".into()))
}

fn find_asset_references(assets_dir: &Path, script_guid: &str) -> Vec<AssetReference> {
    WalkDir::new(assets_dir)
        .into_iter()
        .filter_map(|e| find_references_in_entry(e.ok()?.path(), script_guid))
        .flatten()
        .collect()
}

fn find_references_in_entry(entry: &Path, script_guid: &str) -> Option<Vec<AssetReference>> {
    let ext = entry.extension().and_then(|e| e.to_str());

    if !ext.is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e)) {
        return None;
    }

    let file = File::open(entry).ok()?;
    let references = BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(lnum, l)| {
            l.ok()?.contains(script_guid).then(|| AssetReference {
                path: entry.to_path_buf(),
                line_number: lnum as u32,
            })
        })
        .collect::<Vec<AssetReference>>();

    if references.is_empty() {
        None
    } else {
        Some(references)
    }
}

fn parse_class_line(content: &str) -> Option<u32> {
    let mut parser = Parser::new();
    let language = tree_sitter_c_sharp::LANGUAGE;
    parser
        .set_language(&language.into())
        .expect("Error loading CSharp parser");

    let tree = parser.parse(content, None)?;
    let root = tree.root_node();

    find_class_node(root).map(|node| node.start_position().row as u32)
}

fn find_class_node(node: Node) -> Option<Node> {
    if node.kind() == "class_declaration" {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_class_node(child) {
            return Some(found);
        }
    }

    None
}
