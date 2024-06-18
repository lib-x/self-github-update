#![cfg_attr(feature = "cargo-clippy", deny(clippy::all))]
/*!

[![Build status](https://ci.appveyor.com/api/projects/status/xlkq8rd73cla4ixw/branch/master?svg=true)](https://ci.appveyor.com/project/jaemk/self-update/branch/master)
[![Build Status](https://travis-ci.org/jaemk/self_update.svg?branch=master)](https://travis-ci.org/jaemk/self_update)
[![crates.io:clin](https://img.shields.io/crates/v/self_update.svg?label=self_update)](https://crates.io/crates/self_update)
[![docs](https://docs.rs/self_update/badge.svg)](https://docs.rs/self_update)


`self_update` provides updaters for updating rust executables in-place from various release
distribution backends.

## Usage

Update (replace) the current executable with the latest release downloaded
from `https://api.github.com/repos/jaemk/self_update/releases/latest`.
Note, the [`trust`](https://github.com/japaric/trust) project provides a nice setup for
producing release-builds via CI (travis/appveyor).

### Features

The following [cargo features](https://doc.rust-lang.org/cargo/reference/manifest.html#the-features-section) are
available (but _disabled_ by default):

* `archive-tar`: Support for _tar_ archive format;
* `archive-zip`: Support for _zip_ archive format;
* `compression-flate2`: Support for _gzip_ compression;
* `compression-zip-deflate`: Support for _zip_'s _deflate_ compression format;
* `compression-zip-bzip2`: Support for _zip_'s _bzip2_ compression format;
* `rustls`: Use [pure rust TLS implementation](https://github.com/ctz/rustls) for network requests. This feature does _not_ support 32bit macOS;
* `signatures`: Use [zipsign](https://github.com/Kijewski/zipsign) to verify `.zip` and `.tar.gz` artifacts. Artifacts are assumed to have been signed using zipsign.

Please activate the feature(s) needed by your release files.

### Example

Run the following example to see `self_update` in action:

`cargo run --example github --features "archive-tar archive-zip compression-flate2 compression-zip-deflate"`.

There's also an equivalent example for gitlab:

`cargo run --example gitlab --features "archive-tar archive-zip compression-flate2 compression-zip-deflate"`.

which runs something roughly equivalent to:

```rust
* use self_github_update_enhanced::cargo_crate_version;
*
* fn update() -> Result<(), Box<dyn ::std::error::Error>> {
*     let status = self_github_update_enhanced::backends::github::Update::configure()
*         .repo_owner("jaemk")
*         .repo_name("self_update")
*         .bin_name("github")
*         .show_download_progress(true)
*         .current_version(cargo_crate_version!())
*         .build()?
*         .update()?;
*     println!("Update status: `{}`!", status.version());
*     Ok(())
* }
* ```



Separate utilities are also exposed (**NOTE**: the following example _requires_ the `archive-tar` feature,
see the [features](#features) section above). The `self_replace` crate is re-exported for convenience:

```rust
# #[cfg(feature = "archive-tar")]
fn update() -> Result<(), Box<::std::error::Error>> {
    let releases = self_github_update_enhanced::backends::github::ReleaseList::configure()
        .repo_owner("jaemk")
        .repo_name("self_update")
        .build()?
        .fetch()?;
    println!("found releases:");
    println!("{:#?}\n", releases);

    // get the first available release
    let asset = releases[0]
        .asset_for(&self_update::get_target()).unwrap();

    let tmp_dir = tempfile::Builder::new()
            .prefix("self_update")
            .tempdir_in(::std::env::current_dir()?)?;
    let tmp_tarball_path = tmp_dir.path().join(&asset.name);
    let tmp_tarball = ::std::fs::File::open(&tmp_tarball_path)?;

    self_update::Download::from_url(&asset.download_url)
        .set_header(reqwest::header::ACCEPT, "application/octet-stream".parse()?)
        .download_to(&tmp_tarball)?;

    let bin_name = std::path::PathBuf::from("self_update_bin");
    self_update::Extract::from_source(&tmp_tarball_path)
        .archive(self_update::ArchiveKind::Tar(Some(self_update::Compression::Gz)))
        .extract_file(&tmp_dir.path(), &bin_name)?;

    let new_exe = tmp_dir.path().join(bin_name);
    self_replace::self_replace(new_exe)?;

    Ok(())
}
```

*/

pub use self_replace;
pub use tempfile::TempDir;


#[cfg(feature = "compression-flate2")]
use either::Either;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header;

#[cfg(feature = "client-impersonate")]
use reqwest_impersonate as reqwest;
use std::cmp::min;
use std::fs;
use std::io;
use std::path;
use reqwest::header::HeaderMap;

#[macro_use]
extern crate log;

#[macro_use]
mod macros;
pub mod backends;
pub mod errors;
pub mod update;
pub mod version;

pub const DEFAULT_PROGRESS_TEMPLATE: &str =
    "[{elapsed_precise}] [{bar:40}] {bytes}/{total_bytes} ({eta}) {msg}";
pub const DEFAULT_PROGRESS_CHARS: &str = "=>-";

use errors::*;

/// Get the current target triple.
///
/// Returns a target triple (e.g. `x86_64-unknown-linux-gnu` or `i686-pc-windows-msvc`)
pub fn get_target() -> &'static str {
    env!("TARGET")
}

/// Check if a version tag is greater than the current
#[deprecated(
    since = "0.4.2",
    note = "`should_update` functionality has been moved to `version::bump_is_greater`.\
            `version::bump_is_compatible` should be used instead."
)]
pub fn should_update(current: &str, latest: &str) -> Result<bool> {
    use semver::Version;
    Ok(Version::parse(latest)? > Version::parse(current)?)
}

/// Flush a message to stdout and check if they respond `yes`.
/// Interprets a blank response as yes.
///
/// * Errors:
///     * Io flushing
///     * User entered anything other than enter/Y/y
fn confirm(msg: &str) -> Result<()> {
    print_flush!("{}", msg);

    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    let s = s.trim().to_lowercase();
    if !s.is_empty() && s != "y" {
        bail!(Error::Update, "Update aborted");
    }
    Ok(())
}

/// Status returned after updating
///
/// Wrapped `String`s are version tags
#[derive(Debug, Clone)]
pub enum Status {
    UpToDate(String),
    Updated(String),
}
impl Status {
    /// Return the version tag
    pub fn version(&self) -> &str {
        use Status::*;
        match *self {
            UpToDate(ref s) => s,
            Updated(ref s) => s,
        }
    }

    /// Returns `true` if `Status::UpToDate`
    pub fn uptodate(&self) -> bool {
        matches!(*self, Status::UpToDate(_))
    }

    /// Returns `true` if `Status::Updated`
    pub fn updated(&self) -> bool {
        matches!(*self, Status::Updated(_))
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        use Status::*;
        match *self {
            UpToDate(ref s) => write!(f, "UpToDate({})", s),
            Updated(ref s) => write!(f, "Updated({})", s),
        }
    }
}

/// Supported archive formats
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArchiveKind {
    #[cfg(feature = "archive-tar")]
    Tar(Option<Compression>),
    Plain(Option<Compression>),
    #[cfg(feature = "archive-zip")]
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Compression {
    Gz,
}

fn detect_archive(path: &path::Path) -> Result<ArchiveKind> {
    let ext = path.extension();

    debug!("Detecting archive type using extension: {:?}", ext);

    let res = match ext {
        Some(extension) if extension == std::ffi::OsStr::new("zip") => {
            #[cfg(feature = "archive-zip")]
            {
                debug!("Detected .zip archive");
                Ok(ArchiveKind::Zip)
            }
            #[cfg(not(feature = "archive-zip"))]
            {
                Err(Error::ArchiveNotEnabled("zip".to_string()))
            }
        }
        Some(extension) if extension == std::ffi::OsStr::new("tar") => {
            #[cfg(feature = "archive-tar")]
            {
                debug!("Detected .tar archive");
                Ok(ArchiveKind::Tar(None))
            }
            #[cfg(not(feature = "archive-tar"))]
            {
                Err(Error::ArchiveNotEnabled("tar".to_string()))
            }
        }
        Some(extension) if extension == std::ffi::OsStr::new("tgz") => {
            #[cfg(feature = "archive-tar")]
            {
                debug!("Detected .tgz archive");
                Ok(ArchiveKind::Tar(Some(Compression::Gz)))
            }
            #[cfg(not(feature = "archive-tar"))]
            {
                Err(Error::ArchiveNotEnabled("tar".to_string()))
            }
        }
        Some(extension) if extension == std::ffi::OsStr::new("gz") => match path
            .file_stem()
            .map(path::Path::new)
            .and_then(|f| f.extension())
        {
            Some(extension) if extension == std::ffi::OsStr::new("tar") => {
                #[cfg(feature = "archive-tar")]
                {
                    debug!("Detected .tar.gz archive");
                    Ok(ArchiveKind::Tar(Some(Compression::Gz)))
                }
                #[cfg(not(feature = "archive-tar"))]
                {
                    Err(Error::ArchiveNotEnabled("tar".to_string()))
                }
            }
            _ => Ok(ArchiveKind::Plain(Some(Compression::Gz))),
        },
        _ => Ok(ArchiveKind::Plain(None)),
    };

    debug!("Detected archive type: {:?}", res);

    res
}

/// Extract contents of an encoded archive (e.g. tar.gz) file to a specified directory
///
/// * Errors:
///     * Io - opening files
///     * Io - gzip decoding
///     * Io - archive unpacking
#[derive(Debug)]
pub struct Extract<'a> {
    source: &'a path::Path,
    archive: Option<ArchiveKind>,
}
#[cfg(feature = "compression-flate2")]
pub type GetArchiveReaderResult = Either<fs::File, flate2::read::GzDecoder<fs::File>>;
#[cfg(not(feature = "compression-flate2"))]
pub type GetArchiveReaderResult = fs::File;

impl<'a> Extract<'a> {
    /// Create an `Extract`or from a source path
    pub fn from_source(source: &'a path::Path) -> Extract<'a> {
        Self {
            source,
            archive: None,
        }
    }

    /// Specify an archive format of the source being extracted. If not specified, the
    /// archive format will determined from the file extension.
    pub fn archive(&mut self, kind: ArchiveKind) -> &mut Self {
        self.archive = Some(kind);
        self
    }

    #[allow(unused_variables)]
    fn get_archive_reader(
        source: fs::File,
        compression: Option<Compression>,
    ) -> GetArchiveReaderResult {
        #[cfg(feature = "compression-flate2")]
        match compression {
            Some(Compression::Gz) => Either::Right(flate2::read::GzDecoder::new(source)),
            None => Either::Left(source),
        }
        #[cfg(not(feature = "compression-flate2"))]
        source
    }

    /// Extract an entire source archive into a specified path. If the source is a single compressed
    /// file and not an archive, it will be extracted into a file with the same name inside of
    /// `into_dir`.
    pub fn extract_into(&self, into_dir: &path::Path) -> Result<()> {
        let source = fs::File::open(self.source)?;
        let archive = match self.archive {
            Some(archive) => archive,
            None => detect_archive(self.source)?,
        };

        // We cannot use a feature flag in a match arm. To bypass this the code block is
        // isolated in a closure and called accordingly.
        let extract_into_plain_or_tar = |source: fs::File, compression: Option<Compression>| {
            let mut reader = Self::get_archive_reader(source, compression);

            match archive {
                ArchiveKind::Plain(_) => {
                    match fs::create_dir_all(into_dir) {
                        Ok(_) => (),
                        Err(e) => {
                            if e.kind() != io::ErrorKind::AlreadyExists {
                                return Err(Error::Io(e));
                            }
                        }
                    }
                    let file_name = self
                        .source
                        .file_name()
                        .ok_or_else(|| Error::Update("Extractor source has no file-name".into()))?;
                    let mut out_path = into_dir.join(file_name);
                    out_path.set_extension("");
                    let mut out_file = fs::File::create(&out_path)?;
                    io::copy(&mut reader, &mut out_file)?;
                }
                #[cfg(feature = "archive-tar")]
                ArchiveKind::Tar(_) => {
                    let mut archive = tar::Archive::new(reader);
                    archive.unpack(into_dir)?;
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(
                    "detect_archive() returns in case the proper feature flag is not enabled"
                ),
            };

            Ok(())
        };

        match archive {
            #[cfg(feature = "archive-tar")]
            ArchiveKind::Plain(compression) | ArchiveKind::Tar(compression) => {
                extract_into_plain_or_tar(source, compression)?;
            }
            #[cfg(not(feature = "archive-tar"))]
            ArchiveKind::Plain(compression) => {
                extract_into_plain_or_tar(source, compression)?;
            }
            #[cfg(feature = "archive-zip")]
            ArchiveKind::Zip => {
                let mut archive = zip::ZipArchive::new(source)?;
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i)?;

                    let output_path = into_dir.join(file.name());
                    if let Some(parent_dir) = output_path.parent() {
                        if let Err(e) = fs::create_dir_all(parent_dir) {
                            if e.kind() != io::ErrorKind::AlreadyExists {
                                return Err(Error::Io(e));
                            }
                        }
                    }

                    let mut output = fs::File::create(output_path)?;
                    io::copy(&mut file, &mut output)?;
                }
            }
        };
        Ok(())
    }

    /// Extract a single file from a source and save to a file of the same name in `into_dir`.
    /// If the source is a single compressed file, it will be saved with the name `file_to_extract`
    /// in the specified `into_dir`.
    pub fn extract_file<T: AsRef<path::Path>>(
        &self,
        into_dir: &path::Path,
        file_to_extract: T,
    ) -> Result<()> {
        let file_to_extract = file_to_extract.as_ref();
        let source = fs::File::open(self.source)?;
        let archive = match self.archive {
            Some(archive) => archive,
            None => detect_archive(self.source)?,
        };

        debug!(
            "Attempting to extract {:?} file from {:?}",
            file_to_extract, self.source
        );

        // We cannot use a feature flag in a match arm. To bypass this the code block is
        // isolated in a closure and called accordingly.
        let extract_file_plain_or_tar = |source: fs::File, compression: Option<Compression>| {
            let mut reader = Self::get_archive_reader(source, compression);

            match archive {
                ArchiveKind::Plain(_) => {
                    debug!("Copying file directly");
                    match fs::create_dir_all(into_dir) {
                        Ok(_) => (),
                        Err(e) => {
                            if e.kind() != io::ErrorKind::AlreadyExists {
                                return Err(Error::Io(e));
                            }
                        }
                    }
                    let file_name = file_to_extract
                        .file_name()
                        .ok_or_else(|| Error::Update("Extractor source has no file-name".into()))?;
                    let out_path = into_dir.join(file_name);
                    let mut out_file = fs::File::create(out_path)?;
                    io::copy(&mut reader, &mut out_file)?;
                }
                #[cfg(feature = "archive-tar")]
                ArchiveKind::Tar(_) => {
                    debug!("Extracting from tar");

                    let mut archive = tar::Archive::new(reader);
                    let mut entry = archive
                        .entries()?
                        .filter_map(|e| e.ok())
                        .find(|e| {
                            let p = e.path();
                            debug!("Archive path: {:?}", p);
                            p.ok().filter(|p| p == file_to_extract).is_some()
                        })
                        .ok_or_else(|| {
                            Error::Update(format!(
                                "Could not find the required path in the archive: {:?}",
                                file_to_extract
                            ))
                        })?;
                    entry.unpack_in(into_dir)?;
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(
                    "detect_archive() returns in case the proper feature flag is not enabled"
                ),
            };

            Ok(())
        };

        match archive {
            #[cfg(feature = "archive-tar")]
            ArchiveKind::Plain(compression) | ArchiveKind::Tar(compression) => {
                extract_file_plain_or_tar(source, compression)?;
            }
            #[cfg(not(feature = "archive-tar"))]
            ArchiveKind::Plain(compression) => {
                extract_file_plain_or_tar(source, compression)?;
            }
            #[cfg(feature = "archive-zip")]
            ArchiveKind::Zip => {
                let mut archive = zip::ZipArchive::new(source)?;
                let mut file = archive.by_name(file_to_extract.to_str().unwrap())?;

                let output_path = into_dir.join(file.name());
                if let Some(parent_dir) = output_path.parent() {
                    if let Err(e) = fs::create_dir_all(parent_dir) {
                        if e.kind() != io::ErrorKind::AlreadyExists {
                            return Err(Error::Io(e));
                        }
                    }
                }

                let mut output = fs::File::create(output_path)?;
                io::copy(&mut file, &mut output)?;
            }
        };
        Ok(())
    }
}

/// Moves a file from the given path to the specified destination.
///
/// `source` and `dest` must be on the same filesystem.
/// If `replace_using_temp` is specified, the destination file will be
/// replaced using the given temporary path.
/// If the existing `dest` file is a currently running long running program,
/// `replace_using_temp` may run into errors cleaning up the temp dir.
/// If that's the case for your use-case, consider not specifying a temp dir to use.
///
/// * Errors:
///     * Io - copying / renaming
#[derive(Debug)]
pub struct Move<'a> {
    source: &'a path::Path,
    temp: Option<&'a path::Path>,
}
impl<'a> Move<'a> {
    /// Specify source file
    pub fn from_source(source: &'a path::Path) -> Move<'a> {
        Self { source, temp: None }
    }

    /// If specified and the destination file already exists, the "destination"
    /// file will be moved to the given temporary location before the "source"
    /// file is moved to the "destination" file.
    ///
    /// In the event of an `io` error while renaming "source" to "destination",
    /// the temporary file will be moved back to "destination".
    ///
    /// The `temp` dir must be explicitly provided since `rename` operations require
    /// files to live on the same filesystem.
    pub fn replace_using_temp(&mut self, temp: &'a path::Path) -> &mut Self {
        self.temp = Some(temp);
        self
    }

    /// Move source file to specified destination
    pub fn to_dest(&self, dest: &path::Path) -> Result<()> {
        match self.temp {
            None => {
                fs::rename(self.source, dest)?;
            }
            Some(temp) => {
                if dest.exists() {
                    // Move the existing dest to a temp location so we can move it
                    // back it there's an error. If the existing `dest` file is a
                    // long running program, this may prevent the temp dir from
                    // being cleaned up.
                    fs::rename(dest, temp)?;
                    if let Err(e) = fs::rename(self.source, dest) {
                        fs::rename(temp, dest)?;
                        return Err(Error::from(e));
                    }
                } else {
                    fs::rename(self.source, dest)?;
                }
            }
        };
        Ok(())
    }
}

/// Download things into files
///
/// With optional progress bar
#[derive(Debug)]
pub struct Download {
    show_progress: bool,
    url: String,
    headers: reqwest::header::HeaderMap,
    progress_template: String,
    progress_chars: String,
}
impl Download {
    /// Specify download url
    pub fn from_url(url: &str) -> Self {
        Self {
            show_progress: false,
            url: url.to_owned(),
            headers: reqwest::header::HeaderMap::new(),
            progress_template: DEFAULT_PROGRESS_TEMPLATE.to_string(),
            progress_chars: DEFAULT_PROGRESS_CHARS.to_string(),
        }
    }

    /// Toggle download progress bar
    pub fn show_progress(&mut self, b: bool) -> &mut Self {
        self.show_progress = b;
        self
    }

    /// Set the progress style
    pub fn set_progress_style(
        &mut self,
        progress_template: String,
        progress_chars: String,
    ) -> &mut Self {
        self.progress_template = progress_template;
        self.progress_chars = progress_chars;
        self
    }

    /// Set the download request headers, replaces the existing `HeaderMap`
    pub fn set_headers(&mut self, headers: HeaderMap) -> &mut Self {
        self.headers = headers;
        self
    }

    /// Set a download request header, inserts into the existing `HeaderMap`
    pub fn set_header(
        &mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> &mut Self {
        self.headers.insert(name, value);
        self
    }

    /// Download the file behind the given `url` into the specified `dest`.
    /// Show a sliding progress bar if specified.
    /// If the resource doesn't specify a content-length, the progress bar will not be shown
    ///
    /// * Errors:
    ///     * `reqwest` network errors
    ///     * Unsuccessful response status
    ///     * Progress-bar errors
    ///     * Reading from response to `BufReader`-buffer
    ///     * Writing from `BufReader`-buffer to `File`
    pub fn download_to<T: io::Write>(&self, mut dest: T) -> Result<()> {
        use io::BufRead;
        let mut headers = self.headers.clone();
        if !headers.contains_key(header::USER_AGENT.as_str()) {
            headers.insert(
                header::USER_AGENT.as_str(),
                "rust-reqwest/self-update"
                    .parse()
                    .expect("invalid user-agent"),
            );
        }

        set_ssl_vars!();
        let resp = reqwest::blocking::Client::new()
            .get(&self.url)
            .headers(headers)
            .send()?;
        let size = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .map(|val| {
                val.to_str()
                    .map(|s| s.parse::<u64>().unwrap_or(0))
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        if !resp.status().is_success() {
            bail!(
                Error::Update,
                "Download request failed with status: {:?}",
                resp.status()
            )
        }
        let show_progress = if size == 0 { false } else { self.show_progress };

        let mut src = io::BufReader::new(resp);
        let mut downloaded = 0;
        let mut bar = if show_progress {
            let pb = ProgressBar::new(size);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(&self.progress_template)
                    .expect("set ProgressStyle template failed")
                    .progress_chars(&self.progress_chars),
            );

            Some(pb)
        } else {
            None
        };
        loop {
            let n = {
                let buf = src.fill_buf()?;
                dest.write_all(buf)?;
                buf.len()
            };
            if n == 0 {
                break;
            }
            src.consume(n);
            downloaded = min(downloaded + n as u64, size);

            if let Some(ref mut bar) = bar {
                bar.set_position(downloaded);
            }
        }
        if let Some(ref mut bar) = bar {
            bar.finish_with_message("Done");
        }
        Ok(())
    }
}
