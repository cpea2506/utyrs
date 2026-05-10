use fxhash::FxHashMap;
use gen_lsp_types::Uri;

/// Open document text snapshots keyed by URI.
#[derive(Debug, Default)]
pub struct DocumentStorage {
    documents: FxHashMap<Uri, String>,
}

impl DocumentStorage {
    /// Create empty document storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store or replace full document content.
    pub fn open(&mut self, uri: Uri, content: String) {
        self.documents.insert(uri, content);
    }

    /// Replace content for already-open document.
    pub fn change(&mut self, uri: Uri, change: String) {
        self.documents.insert(uri, change);
    }

    /// Remove document snapshot.
    pub fn close(&mut self, uri: &Uri) {
        self.documents.remove(uri);
    }

    /// Get current snapshot text.
    pub fn get(&self, uri: &Uri) -> Option<&str> {
        self.documents.get(uri).map(|x| x.as_str())
    }
}
