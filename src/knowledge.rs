//! Immutable snapshots: fsync content first, then atomically publish version + main.
use crate::store::{Store, now};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub bytes: u64,
    pub sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    pub id: String,
    pub owner: String,
    pub title: String,
    pub kind: String,
    pub created_at: i64,
    pub main: Option<String>,
    pub shared: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    pub object_id: String,
    pub parent_id: Option<String>,
    pub note: String,
    pub created_at: i64,
    pub content_type: String,
    pub manifest: Vec<Entry>,
}
pub enum Content<'a> {
    Path(&'a Path),
    Text(&'a str),
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn valid_relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains('\\') && !path.contains(':') && !path.contains('\0'),
        "unsafe manifest path"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "unsafe manifest path"
    );
    ensure!(
        !path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".."),
        "unsafe manifest path"
    );
    Ok(())
}
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
fn plain_open(path: &Path) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    Ok(opts.open(path)?)
}
fn signature(m: &fs::Metadata) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}:{:?}:{}:{}",
            m.dev(),
            m.ino(),
            m.len(),
            m.modified()?,
            m.ctime(),
            m.ctime_nsec()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!("{}:{:?}", m.len(), m.modified()?))
    }
}

impl Store {
    pub fn set_identity(&self, member: &str, name: &str, team: &str) -> Result<()> {
        for value in [member, team] {
            ensure!(
                !value.is_empty()
                    && value.len() <= 64
                    && value
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                "member/team IDs must be ASCII letters, numbers, '_' or '-'"
            );
        }
        ensure!(
            !name.trim().is_empty() && name.len() <= 256,
            "invalid display name"
        );
        let old: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT member_id,team_id FROM identity WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, t)) = old {
            ensure!(
                id == member && t == team,
                "persistent identity cannot be replaced"
            );
        }
        self.conn.execute("INSERT INTO identity(singleton,member_id,display_name,team_id) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET display_name=excluded.display_name",params![member,name,team])?;
        Ok(())
    }
    pub fn owner(&self) -> Result<String> {
        self.conn
            .query_row(
                "SELECT member_id FROM identity WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .context("configure a member identity first")
    }
    pub fn create_object(&self, title: &str, kind: &str, shared: bool) -> Result<Object> {
        ensure!(
            !title.trim().is_empty()
                && title.len() <= 512
                && !kind.trim().is_empty()
                && kind.len() <= 64,
            "invalid object title or type"
        );
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO objects(id,owner,title,kind,created_at,shared) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id, self.owner()?, title, kind, now(), shared],
        )?;
        self.object(&id)
    }
    pub fn object(&self, id: &str) -> Result<Object> {
        self.conn
            .query_row(
                "SELECT id,owner,title,kind,created_at,main,shared FROM objects WHERE id=?1",
                [id],
                |r| {
                    Ok(Object {
                        id: r.get(0)?,
                        owner: r.get(1)?,
                        title: r.get(2)?,
                        kind: r.get(3)?,
                        created_at: r.get(4)?,
                        main: r.get(5)?,
                        shared: r.get(6)?,
                    })
                },
            )
            .optional()?
            .context("object not found")
    }
    pub fn objects(&self, shared_only: bool) -> Result<Vec<Object>> {
        let mut q = self
            .conn
            .prepare("SELECT id FROM objects WHERE (?1=0 OR shared=1) ORDER BY created_at,id")?;
        q.query_map([shared_only], |r| r.get::<_, String>(0))?
            .map(|id| self.object(&id?))
            .collect()
    }
    pub fn share_object(&self, id: &str, shared: bool) -> Result<()> {
        ensure!(
            self.object(id)?.owner == self.owner()?,
            "only the owner may share an object"
        );
        self.conn.execute(
            "UPDATE objects SET shared=?2 WHERE id=?1",
            params![id, shared],
        )?;
        Ok(())
    }
    pub fn get_version(&self, id: &str) -> Result<Version> {
        let (object_id,parent_id,note,created_at,content_type,manifest):(String,Option<String>,String,i64,String,String)=self.conn.query_row("SELECT object_id,parent_id,note,created_at,content_type,manifest FROM versions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?.context("version not found")?;
        Ok(Version {
            id: id.into(),
            object_id,
            parent_id,
            note,
            created_at,
            content_type,
            manifest: serde_json::from_str(&manifest)?,
        })
    }
    pub fn history(&self, id: &str) -> Result<Vec<Version>> {
        self.object(id)?;
        let mut q = self
            .conn
            .prepare("SELECT id FROM versions WHERE object_id=?1 ORDER BY rowid")?;
        q.query_map([id], |r| r.get::<_, String>(0))?
            .map(|id| self.get_version(&id?))
            .collect()
    }
    pub fn content_root(&self) -> PathBuf {
        let mut p = self.path.as_os_str().to_owned();
        p.push(".content");
        p.into()
    }
    pub fn blob_path(&self, digest: &str) -> Result<PathBuf> {
        ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid content hash"
        );
        Ok(self
            .content_root()
            .join("blobs")
            .join(&digest[..2])
            .join(digest))
    }
    pub(crate) fn write_blob(&self, mut input: impl Read) -> Result<(u64, String)> {
        let root = self.content_root();
        fs::create_dir_all(root.join("staging"))?;
        sync_dir(root.parent().context("content parent missing")?)?;
        let temp = root.join("staging").join(Uuid::new_v4().to_string());
        let result = (|| {
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            let mut digest = Sha256::new();
            let mut count = 0u64;
            let mut buf = [0u8; 65536];
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                digest.update(&buf[..n]);
                count += n as u64;
            }
            out.sync_all()?;
            drop(out);
            let digest = format!("{digest:x}", digest = digest.finalize());
            let dest = self.blob_path(&digest)?;
            let parent = dest.parent().unwrap();
            fs::create_dir_all(parent)?;
            sync_dir(&root)?;
            sync_dir(&root.join("blobs"))?;
            match fs::hard_link(&temp, &dest) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.verify_blob(&Entry {
                        path: "content".into(),
                        bytes: count,
                        sha256: Some(digest.clone()),
                    })?;
                }
                Err(e) => return Err(e.into()),
            }
            sync_dir(parent)?;
            Ok((count, digest))
        })();
        let _ = fs::remove_file(&temp);
        result
    }
    fn snapshot(&self, source: &Path) -> Result<Vec<Entry>> {
        ensure!(
            !fs::symlink_metadata(source)?.file_type().is_symlink(),
            "symbolic links are unsupported"
        );
        let source = self.allowed_source(source)?;
        let mut locks = self.path.as_os_str().to_owned();
        locks.push(".run-locks");
        let mut results = self.path.as_os_str().to_owned();
        results.push(".results");
        let mut service = self.path.as_os_str().to_owned();
        service.push(".service");
        for own in [
            &self.path,
            &self.content_root(),
            &PathBuf::from(locks),
            &PathBuf::from(results),
            &PathBuf::from(service),
        ] {
            ensure!(
                !source.starts_with(own) && !(source.is_dir() && own.starts_with(&source)),
                "source includes xXASSXx database/content/runtime files"
            );
        }
        for suffix in ["-wal", "-shm"] {
            let mut own = self.path.as_os_str().to_owned();
            own.push(suffix);
            ensure!(
                source.as_os_str() != own,
                "cannot snapshot SQLite runtime files"
            );
        }
        let mut entries = Vec::new();
        if source.is_file() {
            let name = source
                .file_name()
                .and_then(|s| s.to_str())
                .context("non-UTF-8 path unsupported")?;
            self.walk_snapshot(&source, name, &mut entries)?;
        } else if source.is_dir() {
            self.walk_snapshot(&source, "", &mut entries)?;
        } else {
            bail!("special files are unsupported");
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        ensure!(entries.len() <= 10000, "snapshot exceeds 10000 entries");
        Ok(entries)
    }
    fn walk_snapshot(&self, path: &Path, relative: &str, entries: &mut Vec<Entry>) -> Result<()> {
        ensure!(entries.len() < 10000, "snapshot exceeds 10000 entries");
        let before = fs::symlink_metadata(path)?;
        ensure!(
            !before.file_type().is_symlink(),
            "symbolic links are unsupported"
        );
        if !relative.is_empty() {
            valid_relative(relative)?;
        }
        if before.is_file() {
            let file = plain_open(path)?;
            ensure!(
                signature(&before)? == signature(&file.metadata()?)?,
                "source changed; retry publication"
            );
            let (bytes, sha256) = self.write_blob(file)?;
            let after = fs::symlink_metadata(path)?;
            // ctime also catches metadata-only changes (for example background
            // filesystem attributes on macOS). Conservatively require a retry.
            ensure!(
                bytes == before.len() && signature(&before)? == signature(&after)?,
                "source changed during read (including metadata); retry publication"
            );
            entries.push(Entry {
                path: relative.into(),
                bytes,
                sha256: Some(sha256),
            });
        } else if before.is_dir() {
            if !relative.is_empty() {
                entries.push(Entry {
                    path: relative.into(),
                    bytes: 0,
                    sha256: None,
                });
            }
            let mut children = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
            children.sort_by_key(|e| e.file_name());
            for child in children {
                let name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("non-UTF-8 path unsupported"))?;
                let rel = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                self.walk_snapshot(&child.path(), &rel, entries)?;
            }
            ensure!(
                signature(&before)? == signature(&fs::symlink_metadata(path)?)?,
                "directory changed during read; retry publication"
            );
        } else {
            bail!("special files are unsupported");
        }
        Ok(())
    }
    pub fn publish(
        &mut self,
        object: &str,
        expected: Option<&str>,
        request: &str,
        note: &str,
        content: Content<'_>,
    ) -> Result<Version> {
        ensure!(
            !request.is_empty() && request.len() <= 128 && note.len() <= 4096,
            "invalid publish request"
        );
        ensure!(
            self.object(object)?.owner == self.owner()?,
            "only owner may publish"
        );
        if let Some(parent) = expected {
            ensure!(
                self.get_version(parent)?.object_id == object,
                "parent belongs to another object"
            );
        }
        let (kind, manifest) = match content {
            Content::Path(p) => ("snapshot", self.snapshot(p)?),
            Content::Text(s) => {
                ensure!(s.len() <= 1024 * 1024, "text exceeds 1 MiB");
                let (bytes, sha256) = self.write_blob(s.as_bytes())?;
                (
                    "text",
                    vec![Entry {
                        path: "record.txt".into(),
                        bytes,
                        sha256: Some(sha256),
                    }],
                )
            }
        };
        let fingerprint = hash(&serde_json::to_vec(&(
            object, expected, note, kind, &manifest,
        ))?);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT id,fingerprint FROM versions WHERE request_id=?1",
                [request],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, old)) = previous {
            ensure!(old == fingerprint, "request_id conflict");
            drop(tx);
            return self.get_version(&id);
        }
        let main: Option<String> =
            tx.query_row("SELECT main FROM objects WHERE id=?1", [object], |r| {
                r.get(0)
            })?;
        ensure!(
            main.as_deref() == expected,
            "publication conflict: main changed"
        );
        let id = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO versions(id,object_id,parent_id,note,created_at,content_type,manifest,request_id,fingerprint) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,object,expected,note,now(),kind,serde_json::to_string(&manifest)?,request,fingerprint])?;
        tx.execute(
            "UPDATE objects SET main=?2 WHERE id=?1",
            params![object, id],
        )?;
        tx.commit()?;
        self.get_version(&id)
    }
    fn verify_blob(&self, e: &Entry) -> Result<()> {
        if let Some(hash) = &e.sha256 {
            let mut f = plain_open(&self.blob_path(hash)?)?;
            ensure!(f.metadata()?.is_file(), "content is not a regular file");
            let mut digest = Sha256::new();
            let mut count = 0;
            let mut buf = [0; 65536];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                digest.update(&buf[..n]);
                count += n as u64;
            }
            ensure!(
                count == e.bytes && format!("{:x}", digest.finalize()) == *hash,
                "content corrupt: {}",
                e.path
            );
        } else {
            ensure!(e.bytes == 0, "invalid directory entry");
        }
        Ok(())
    }
    pub fn verify_version(&self, id: &str) -> Result<Version> {
        let v = self.get_version(id)?;
        let mut seen = BTreeMap::new();
        for e in &v.manifest {
            valid_relative(&e.path)?;
            ensure!(
                seen.insert(e.path.clone(), e.sha256.is_some()).is_none(),
                "duplicate manifest path"
            );
            self.verify_blob(e)?;
        }
        for e in &v.manifest {
            let mut p = Path::new(&e.path).parent();
            while let Some(parent) = p {
                if let Some(s) = parent.to_str() {
                    ensure!(seen.get(s) != Some(&true), "file used as directory");
                }
                p = parent.parent();
            }
        }
        Ok(v)
    }
    pub fn text_version(&self, id: &str) -> Result<String> {
        let v = self.verify_version(id)?;
        ensure!(
            v.content_type == "text" && v.manifest.len() == 1,
            "not a text version"
        );
        Ok(fs::read_to_string(self.blob_path(
            v.manifest[0].sha256.as_deref().context("missing text")?,
        )?)?)
    }
    pub fn export_version(&self, id: &str, target: &Path) -> Result<()> {
        ensure!(
            fs::symlink_metadata(target).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "export destination already exists"
        );
        let v = self.verify_version(id)?;
        fs::create_dir(target)?;
        for e in &v.manifest {
            let dest = target.join(&e.path);
            if e.sha256.is_none() {
                fs::create_dir_all(&dest)?;
                continue;
            }
            if let Some(p) = dest.parent() {
                fs::create_dir_all(p)?;
            }
            let mut input = plain_open(&self.blob_path(e.sha256.as_deref().unwrap())?)?;
            let mut out = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&dest)?;
            let mut digest = Sha256::new();
            let mut bytes = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buffer[..n])?;
                digest.update(&buffer[..n]);
                bytes += n as u64;
            }
            out.sync_all()?;
            ensure!(
                bytes == e.bytes && Some(format!("{:x}", digest.finalize())) == e.sha256,
                "content changed during export; destination is incomplete"
            );
        }
        sync_dir(target)?;
        Ok(())
    }
    pub fn diff_versions(&self, left: &str, right: &str) -> Result<serde_json::Value> {
        let a = self.get_version(left)?;
        let b = self.get_version(right)?;
        ensure!(
            a.object_id == b.object_id,
            "versions belong to different objects"
        );
        let a: BTreeMap<_, _> = a
            .manifest
            .into_iter()
            .map(|e| (e.path.clone(), e))
            .collect();
        let b: BTreeMap<_, _> = b
            .manifest
            .into_iter()
            .map(|e| (e.path.clone(), e))
            .collect();
        Ok(
            serde_json::json!({"added":b.keys().filter(|p|!a.contains_key(*p)).collect::<Vec<_>>(),"removed":a.keys().filter(|p|!b.contains_key(*p)).collect::<Vec<_>>(),"changed":a.iter().filter(|(p,e)|b.get(*p).is_some_and(|r|r!=*e)).map(|(p,_)|p).collect::<Vec<_>>()}),
        )
    }
}
