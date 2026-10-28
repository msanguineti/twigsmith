use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, PartialEq, Eq)]
pub struct FileState {
    file_type: fs::FileType,
    content: Vec<u8>,
    modified: SystemTime,
    permissions: fs::Permissions,
}

pub fn snapshot(root: &Path) -> BTreeMap<PathBuf, FileState> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, FileState>) {
        let metadata = fs::symlink_metadata(path)
            .unwrap_or_else(|error| panic!("snapshot metadata {}: {error}", path.display()));
        let content = if metadata.is_file() {
            fs::read(path)
                .unwrap_or_else(|error| panic!("snapshot read {}: {error}", path.display()))
        } else if metadata.file_type().is_symlink() {
            fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            assert!(metadata.is_dir());
            Vec::new()
        };
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            FileState {
                file_type: metadata.file_type(),
                content,
                modified: metadata.modified().unwrap(),
                permissions: metadata.permissions(),
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
