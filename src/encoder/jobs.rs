use std::path::{Path, PathBuf};

use crate::encoder::{is_media_file, ArchiveType};

/// Одна задача кодирования: один вход → один `.ail`.
#[derive(Debug, Clone)]
pub struct EncodingJob {
    pub input: PathBuf,
    pub output: PathBuf,
    pub label: String,
}

pub fn is_archive(path: &Path) -> bool {
    ArchiveType::from_path(path).is_some()
}

/// Разворачивает выбранные пути в отдельные задания.
/// Каждый архив в папке → отдельный `.ail`.
pub fn collect_encoding_jobs(roots: &[PathBuf], output_dir: Option<&Path>) -> Vec<EncodingJob> {
    let mut jobs = Vec::new();

    for root in roots {
        if root.is_file() {
            push_job(&mut jobs, root, output_dir);
            continue;
        }

        if !root.is_dir() {
            continue;
        }

        let mut archives = Vec::new();
        collect_archives_recursive(root, &mut archives);
        archives.sort();
        archives.dedup();

        if archives.is_empty() {
            push_job(&mut jobs, root, output_dir);
            continue;
        }

        for archive in archives {
            push_job(&mut jobs, &archive, output_dir);
        }

        if folder_has_loose_media(root) {
            push_job(&mut jobs, root, output_dir);
        }
    }

    jobs.sort_by(|a, b| a.input.cmp(&b.input));
    jobs.dedup_by(|a, b| a.input == b.input && a.output == b.output);
    jobs
}

fn push_job(jobs: &mut Vec<EncodingJob>, input: &Path, output_dir: Option<&Path>) {
    let output = output_path_for(input, output_dir);
    let label = input
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| input.display().to_string());

    jobs.push(EncodingJob {
        input: input.to_path_buf(),
        output,
        label,
    });
}

fn output_path_for(input: &Path, output_dir: Option<&Path>) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| {
            input
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        });

    match output_dir {
        Some(dir) => dir.join(format!("{stem}.ail")),
        None if input.is_dir() => input.join(format!(
            "{}.ail",
            input
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        )),
        None => input.with_extension("ail"),
    }
}

fn collect_archives_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_archives_recursive(&path, out);
        } else if is_archive(&path) {
            out.push(path);
        }
    }
}

fn folder_has_loose_media(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_media_file(&path) && !is_archive(&path) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn folder_with_two_archives_produces_two_jobs() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.zip");
        let b = dir.path().join("b.zip");
        fs::write(&a, b"zip").unwrap();
        fs::write(&b, b"zip").unwrap();

        let jobs = collect_encoding_jobs(&[dir.path().to_path_buf()], None);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().any(|j| j.input.ends_with("a.zip")));
        assert!(jobs.iter().any(|j| j.input.ends_with("b.zip")));
    }

    #[test]
    fn single_archive_is_one_job() {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("comic.zip");
        fs::write(&zip, b"zip").unwrap();

        let jobs = collect_encoding_jobs(&[zip], None);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].output, dir.path().join("comic.ail"));
    }
}
