// ~/~ begin <<docs/src/brei.md#brei::src/tasks/history.rs>>[init]
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use super::db::TaskDb;

pub const HISTORY_FILE: &str = ".brei_history";

impl TaskDb {
    /// Read the history from a file, relative to the root.
    pub fn load_history(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let file = match File::open(self.root.join(path)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let data: BTreeMap<PathBuf, String> = serde_json::from_reader(BufReader::new(file))?;
        self.history.lock().unwrap().extend(data);
        Ok(())
    }

    /// Write the history to a file, relative to the root.
    pub fn save_history(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = self.root.join(path);
        let dir = path.parent().unwrap_or(&self.root);
        let mut tmp = NamedTempFile::new_in(dir)?;
        {
            let mut out = BufWriter::new(tmp.as_file_mut());
            serde_json::to_writer_pretty(&mut out, &*self.history.lock().unwrap())?;
            writeln!(out)?;
            out.flush()?;
        }
        tmp.persist(path)?;
        Ok(())
    }
}
// ~/~ end
