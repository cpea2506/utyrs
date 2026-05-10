use gen_lsp_types::{CodeLens, Command, Location, Position, Range, Uri};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use thiserror::Error;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: [&str; 3] = ["unity", "prefab", "asset"];

/// Result type for code lens operations.
pub type CodeLensResult<T> = Result<T, CodeLensError>;

/// Error type for code lens operations.
#[derive(Debug, Error)]
pub enum CodeLensError {
    /// Failed to serialize data to JSON.
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Invalid file URI.
    #[error("invalid file URI: {0}")]
    InvalidUri(Uri),
    /// Invalid file path.
    #[error("invalid file path: {0}")]
    InvalidPath(PathBuf),
    /// Missing required code lens data.
    #[error("missing code lens data")]
    MissingData,
    /// Unable to read metadata file.
    #[error("unable to read metadata file {path}: {source}")]
    MetadataRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Metadata file does not contain a guid line.
    #[error("metadata file {path} does not contain a guid")]
    MetadataMissingGuid { path: PathBuf },
    /// Unable to read metadata file line.
    #[error("unable to read metadata line {line} in {path}: {source}")]
    MetadataLineRead {
        path: PathBuf,
        line: u32,
        #[source]
        source: std::io::Error,
    },
    /// Failed to parse C# source with tree-sitter.
    #[error("parse error: {0}")]
    ParseError(String),
}

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
    pub fn create(
        workspace_root: &Path,
        content: &str,
        script_uri: &Uri,
    ) -> CodeLensResult<Vec<CodeLens>> {
        let assets_dir = workspace_root.join("Assets");

        let class_line = parse_class_line(content)?.unwrap_or(0);
        let asset_references = find_references(&assets_dir, script_uri)?;

        let data = serde_json::to_value(asset_references)?;

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
        let asset_references = serde_json::from_value::<Vec<AssetReference>>(data)?;

        let locations = asset_references
            .iter()
            .map(AssetReference::to_codelens_location)
            .collect::<CodeLensResult<Vec<Location>>>()?;
        let count = locations.len();
        let title = if count == 1 {
            "1 Unity reference".to_string()
        } else {
            format!("{count} Unity references")
        };
        let arguments = serde_json::to_value(locations)?;

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

fn find_references(assets_dir: &Path, script_uri: &Uri) -> CodeLensResult<Vec<AssetReference>> {
    let meta_path = script_uri
        .to_file_path()
        .map_err(|_| CodeLensError::InvalidUri(script_uri.clone()))?
        .with_extension("cs.meta");

    let script_guid = extract_guid_from_meta(&meta_path)?;

    find_asset_references(assets_dir, &script_guid)
}

fn extract_guid_from_meta(meta_path: &Path) -> CodeLensResult<String> {
    /* First two lines of Unity meta file:
     *
     * fileFormatVersion: 2
     * guid: 83c14770bb100154e969c8bc1a4f153c
     * */

    let file = File::open(meta_path).map_err(|source| CodeLensError::MetadataRead {
        path: meta_path.to_path_buf(),
        source,
    })?;

    for (line, line_result) in BufReader::new(file).lines().enumerate() {
        let line = line_result.map_err(|source| CodeLensError::MetadataLineRead {
            path: meta_path.to_path_buf(),
            line: line as u32,
            source,
        })?;

        if let Some(guid) = line.strip_prefix("guid: ") {
            return Ok(guid.to_owned());
        }
    }

    Err(CodeLensError::MetadataMissingGuid {
        path: meta_path.to_path_buf(),
    })
}

fn find_asset_references(
    assets_dir: &Path,
    script_guid: &str,
) -> CodeLensResult<Vec<AssetReference>> {
    let mut references = Vec::new();

    for entry in WalkDir::new(assets_dir).into_iter().filter_map(Result::ok) {
        if let Some(mut found) = find_references_in_entry(entry.path(), script_guid) {
            references.append(&mut found);
        }
    }

    references.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line_number.cmp(&right.line_number))
    });

    Ok(references)
}

fn find_references_in_entry(
    entry: &Path,
    script_guid: &str,
) -> Option<Vec<AssetReference>> {
    let ext = entry.extension().and_then(|e| e.to_str());

    if !ext.is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e)) {
        return None;
    }

    let file = File::open(entry).ok()?;
    let mut references = Vec::new();

    for (lnum, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            continue;
        };

        if line.contains(script_guid) {
            references.push(AssetReference {
                path: entry.to_path_buf(),
                line_number: lnum as u32,
            });
        }
    }

    if references.is_empty() {
        None
    } else {
        Some(references)
    }
}

fn parse_class_line(content: &str) -> CodeLensResult<Option<u32>> {
    let mut parser = Parser::new();
    let language = tree_sitter_c_sharp::LANGUAGE.into();
    parser
        .set_language(&language)
        .map_err(|e| CodeLensError::ParseError(format!("failed to load C# parser: {e}")))?;

    let tree = parser
        .parse(content, None)
        .ok_or_else(|| CodeLensError::ParseError("failed to parse C# document".into()))?;
    let root = tree.root_node();
    let query = Query::new(&language, "(class_declaration) @class")
        .map_err(|e| CodeLensError::ParseError(format!("failed to compile C# query: {e}")))?;
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, root, content.as_bytes());

    Ok(captures
        .next()
        .map(|(query_match, capture_index)| query_match.captures[*capture_index].node)
        .map(|node| node.start_position().row as u32))
}
