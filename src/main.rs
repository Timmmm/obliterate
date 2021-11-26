use anyhow::{anyhow, bail, Result};
use std::{
    fs, io, os,
    path::{Path, PathBuf},
};
use structopt::StructOpt;
use walkdir::WalkDir;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "obliterate",
    about = "Remove a directory tree. Unlike `rm` if some files or directories are read-only it will try to make them writable."
)]
struct Opt {
    paths: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();

    for path in opt.paths {
        // Errors are printed individually for each file, we do not care
        // if there was an error overall; it is only used for testing.
        let _ = remove_path(&path);
    }

    Ok(())
}

/// We have to use different functions to remove files or directories, so we
/// use this to tell `remove_file_or_dir` which kind we are deleting.
enum FileOrDir {
    File,
    Dir,
}

/// Remove the entire directory tree (or file) at `path`.
fn remove_path(path: &Path) -> Result<()> {
    let mut success = true;

    for entry in WalkDir::new(path).contents_first(true).into_iter() {
        match entry {
            Ok(entry) => {
                if let Err(e) = remove_file_or_dir(
                    entry.path(),
                    if entry.file_type().is_dir() {
                        FileOrDir::Dir
                    } else {
                        FileOrDir::File
                    },
                ) {
                    eprintln!("Error removing '{}': {}", entry.path().display(), e);
                    success = false;
                }
            }
            Err(e) => {
                eprintln!("Error accessing file: {}", e);
                success = false;
            }
        }
    }

    if !success {
        bail!("One or more errors deleting '{}'", path.display());
    }
    Ok(())
}

/// Delete a single file or directory.
fn remove_file_or_dir(path: &Path, file_or_dir: FileOrDir) -> Result<()> {
    // The function to use for deletion.
    let remove_item = match file_or_dir {
        FileOrDir::File => fs::remove_file,
        FileOrDir::Dir => fs::remove_dir,
    };

    // Try to delete the file or directory. Return success/failure unless it
    // failed with `PermissionDenied`.
    let original_error = match remove_item(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => e,
        Err(e) => return Err(e.into()),
    };

    // Permission was denied. Check if it's a permission we can grant. On Unix
    // we need to do `chmod u+w` on the parent directory. On Windows we need
    // to clear the read only attribute on the file itself.
    // See `FILE_ATTRIBUTE_READONLY` here: https://docs.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants
    //
    // Note that it says that the attribute is ignored for directories, but that
    // does not seem to be the case based on my testing. See
    // https://support.microsoft.com/en-gb/topic/you-cannot-view-or-change-the-read-only-or-the-system-attributes-of-folders-in-windows-server-2003-in-windows-xp-in-windows-vista-or-in-windows-7-55bd5ec5-d19e-6173-0df1-8f5b49247165
    //
    // Windows also has a proper ACL system but we don't try to use it.

    let permission_target = path_to_make_writable(path, file_or_dir)?;

    let metadata = match permission_target.metadata() {
        Ok(m) => m,
        Err(e) => {
            bail!(
                "Permission denied, additionally an error occured when reading the metadata of '{}': {}",
                permission_target.display(),
                e,
            );
        }
    };
    let mut permissions = metadata.permissions();

    if is_writable(&permissions) {
        // The file/directory (or parent directory on Unix) were writable but
        // we got permission denied anyway.
        return Err(original_error.into());
    }

    // Set it (or the parent directory on Unix) as writable.
    set_writable(&mut permissions);
    match fs::set_permissions(permission_target, permissions) {
        Ok(_) => {}
        Err(e) => {
            // Give up.
            bail!(
                "Error setting '{}' to be writable: {}",
                permission_target.display(),
                e
            );
        }
    }
    // Try deleting it one last time.
    Ok(remove_item(path)?)
}

#[cfg(unix)]
fn set_writable(permissions: &mut fs::Permissions) {
    use os::unix::prelude::PermissionsExt;
    // The default `set_readonly()` weirdly sets it on the "all" part, which
    // means if the delete fails we leave files writable by everyone. Probably
    // not what was intended. Set the mode on the "user" part explicitly instead.
    permissions.set_mode(permissions.mode() | 0o200);
}

#[cfg(not(unix))]
fn set_writable(permissions: &mut fs::Permissions) {
    permissions.set_readonly(false);
}

#[cfg(unix)]
fn is_writable(permissions: &fs::Permissions) -> bool {
    use os::unix::prelude::PermissionsExt;
    // The default `readonly()` checks for write bits on any of the user/group/other
    // parts. That doesn't work because a) we might not be in the group, and
    // b) you need user write permissions to delete a file. Group/other aren't enough.
    permissions.mode() & 0o200 != 0
}

#[cfg(not(unix))]
fn is_writable(permissions: &fs::Permissions) -> bool {
    !permissions.readonly()
}

#[cfg(unix)]
fn path_to_make_writable(path: &Path, _file_or_dir: FileOrDir) -> Result<&Path> {
    path.parent().ok_or(anyhow!(
        "Cannot make parent path writable because it is in the root directory"
    ))
}

#[cfg(not(unix))]
fn path_to_make_writable(path: &Path, file_or_dir: FileOrDir) -> Result<&Path> {
    Ok(path)
}

#[cfg(test)]
mod test {
    use super::*;
    use tempdir::TempDir;

    #[test]
    fn simple() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }

    #[test]
    fn readonly_file() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        let file_path = path.join("dir1/dir2/file1");
        let mut permissions = file_path.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        // TODO: set_permissions is weird; it changes the `all` permission not `user`.
        fs::set_permissions(file_path, permissions).unwrap();

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }

    #[test]
    fn readonly_dir() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        let file_path = path.join("dir1");
        let mut permissions = file_path.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(file_path, permissions).unwrap();

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }

    #[test]
    fn readonly_everything() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        for p in ["dir1", "dir1/dir2", "dir1/file1", "dir1/dir2/file1"] {
            let file_path = path.join(p);
            let mut permissions = file_path.metadata().unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(file_path, permissions).unwrap();
        }

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }
}
