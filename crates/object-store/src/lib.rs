use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, ensure, Context, Result};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;
use vpsman_common::{
    create_private_file_new_async, ensure_private_dir_async, ensure_private_dir_tree_async,
    repair_private_file_permissions_async,
};

type HmacSha256 = Hmac<Sha256>;

const S3_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_S3_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;
const S3_SPOOL_DIR: &str = "vpsman-object-store-spool";

#[derive(Clone, Debug)]
pub enum BackupObjectStore {
    Filesystem(FilesystemBackupObjectStore),
    S3(S3BackupObjectStore),
}

#[derive(Debug)]
pub struct VerifiedObjectFile {
    pub path: PathBuf,
    pub cleanup_after_stream: bool,
}

impl BackupObjectStore {
    pub fn filesystem(root: PathBuf) -> Result<Self> {
        Ok(Self::Filesystem(FilesystemBackupObjectStore::new(root)?))
    }

    pub fn s3(settings: S3BackupObjectStoreSettings) -> Result<Self> {
        Ok(Self::S3(S3BackupObjectStore::new(settings)?))
    }

    pub async fn put_new(&self, object_key: &str, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Filesystem(store) => store.put_new(object_key, bytes).await.map(|_| ()),
            Self::S3(store) => store.put_new(object_key, bytes).await,
        }
    }

    pub async fn put_file_idempotent(
        &self,
        object_key: &str,
        source_path: &Path,
        expected_sha256_hex: &str,
        expected_size_bytes: u64,
    ) -> Result<bool> {
        match self {
            Self::Filesystem(store) => {
                store
                    .put_file_idempotent(
                        object_key,
                        source_path,
                        expected_sha256_hex,
                        expected_size_bytes,
                    )
                    .await
            }
            Self::S3(store) => {
                let bytes = tokio::fs::read(source_path).await.with_context(|| {
                    format!(
                        "failed to read source object file {}",
                        source_path.display()
                    )
                })?;
                ensure!(
                    bytes.len() as u64 == expected_size_bytes,
                    "source object size mismatch"
                );
                ensure!(
                    sha256_hex(&bytes) == expected_sha256_hex,
                    "source object hash mismatch"
                );
                match store.put_new(object_key, &bytes).await {
                    Ok(()) => Ok(true),
                    Err(error) => match store
                        .get_with_limit(
                            object_key,
                            usize::try_from(expected_size_bytes).unwrap_or(usize::MAX),
                        )
                        .await
                    {
                        Ok(existing)
                            if existing.len() as u64 == expected_size_bytes
                                && sha256_hex(&existing) == expected_sha256_hex =>
                        {
                            Ok(false)
                        }
                        _ => Err(error),
                    },
                }
            }
        }
    }

    pub async fn get(&self, object_key: &str) -> Result<Vec<u8>> {
        match self {
            Self::Filesystem(store) => store.get(object_key).await,
            Self::S3(store) => store.get(object_key).await,
        }
    }

    pub async fn get_with_limit(&self, object_key: &str, max_bytes: usize) -> Result<Vec<u8>> {
        match self {
            Self::Filesystem(store) => store.get_with_limit(object_key, max_bytes).await,
            Self::S3(store) => store.get_with_limit(object_key, max_bytes).await,
        }
    }

    pub async fn verified_filesystem_path(
        &self,
        object_key: &str,
        expected_sha256_hex: &str,
        expected_size_bytes: u64,
    ) -> Result<Option<PathBuf>> {
        match self {
            Self::Filesystem(store) => store
                .verified_path(object_key, expected_sha256_hex, expected_size_bytes)
                .await
                .map(Some),
            Self::S3(_) => Ok(None),
        }
    }

    pub async fn verified_object_file(
        &self,
        object_key: &str,
        expected_sha256_hex: &str,
        expected_size_bytes: u64,
        max_bytes: usize,
    ) -> Result<VerifiedObjectFile> {
        ensure!(
            expected_size_bytes <= max_bytes as u64,
            "object exceeded {max_bytes} bytes"
        );
        match self {
            Self::Filesystem(store) => Ok(VerifiedObjectFile {
                path: store
                    .verified_path(object_key, expected_sha256_hex, expected_size_bytes)
                    .await?,
                cleanup_after_stream: false,
            }),
            Self::S3(store) => {
                store
                    .spool_verified_to_temp_file(
                        object_key,
                        expected_sha256_hex,
                        expected_size_bytes,
                        max_bytes,
                    )
                    .await
            }
        }
    }

    pub async fn delete_best_effort(&self, object_key: &str) {
        match self {
            Self::Filesystem(store) => store.delete_best_effort(object_key).await,
            Self::S3(store) => store.delete_best_effort(object_key).await,
        }
    }

    pub async fn delete_confirmed(&self, object_key: &str) -> Result<()> {
        match self {
            Self::Filesystem(store) => store.delete_confirmed(object_key).await,
            Self::S3(store) => store.delete_confirmed(object_key).await,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Filesystem(_) => "filesystem",
            Self::S3(_) => "s3",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FilesystemBackupObjectStore {
    root: Arc<PathBuf>,
}

impl FilesystemBackupObjectStore {
    fn new(root: PathBuf) -> Result<Self> {
        ensure!(!root.as_os_str().is_empty(), "object store root is empty");
        Ok(Self {
            root: Arc::new(root),
        })
    }

    async fn put_new(&self, object_key: &str, bytes: &[u8]) -> Result<PathBuf> {
        validate_object_key(object_key)?;
        let path = self.path_for_key(object_key)?;
        self.ensure_private_object_parent(&path).await?;
        ensure!(
            !tokio::fs::try_exists(&path).await.unwrap_or(false),
            "object already exists"
        );
        let temp_path = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let mut file = create_private_file_new_async(&temp_path)
            .await
            .with_context(|| format!("failed to create temp object {}", temp_path.display()))?;
        file.write_all(bytes)
            .await
            .with_context(|| format!("failed to write temp object {}", temp_path.display()))?;
        file.sync_data()
            .await
            .with_context(|| format!("failed to sync temp object {}", temp_path.display()))?;
        match tokio::fs::hard_link(&temp_path, &path).await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Ok(path)
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Err(error).with_context(|| format!("failed to commit object {}", path.display()))
            }
        }
    }

    async fn put_file_idempotent(
        &self,
        object_key: &str,
        source_path: &Path,
        expected_sha256_hex: &str,
        expected_size_bytes: u64,
    ) -> Result<bool> {
        validate_object_key(object_key)?;
        let source_metadata = tokio::fs::metadata(source_path)
            .await
            .with_context(|| format!("failed to stat source object {}", source_path.display()))?;
        ensure!(source_metadata.is_file(), "source object is not a file");
        ensure!(
            source_metadata.len() == expected_size_bytes,
            "source object size mismatch"
        );
        ensure!(
            sha256_file_hex(source_path).await? == expected_sha256_hex,
            "source object hash mismatch"
        );
        let path = self.path_for_key(object_key)?;
        self.ensure_private_object_parent(&path).await?;
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            repair_private_file_permissions_async(&path)
                .await
                .with_context(|| format!("failed to secure object {}", path.display()))?;
            ensure!(
                tokio::fs::metadata(&path).await?.len() == expected_size_bytes
                    && sha256_file_hex(&path).await? == expected_sha256_hex,
                "object already exists with different contents"
            );
            return Ok(false);
        }
        let temp_path = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let mut source = tokio::fs::File::open(source_path)
            .await
            .with_context(|| format!("failed to open source object {}", source_path.display()))?;
        let mut file = create_private_file_new_async(&temp_path)
            .await
            .with_context(|| format!("failed to create temp object {}", temp_path.display()))?;
        tokio::io::copy(&mut source, &mut file)
            .await
            .with_context(|| {
                format!(
                    "failed to copy source object {} to temp object {}",
                    source_path.display(),
                    temp_path.display()
                )
            })?;
        file.sync_data()
            .await
            .with_context(|| format!("failed to sync temp object {}", temp_path.display()))?;
        match tokio::fs::hard_link(&temp_path, &path).await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Ok(true)
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Err(error).with_context(|| format!("failed to commit object {}", path.display()))
            }
        }
    }

    async fn get(&self, object_key: &str) -> Result<Vec<u8>> {
        self.get_with_limit(object_key, usize::MAX).await
    }

    async fn get_with_limit(&self, object_key: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let path = self.path_for_key(object_key)?;
        let metadata = self.secure_existing_object_file(&path).await?;
        ensure!(
            metadata.len() <= max_bytes as u64,
            "object exceeded {max_bytes} bytes"
        );
        tokio::fs::read(&path)
            .await
            .with_context(|| format!("failed to read object {}", path.display()))
    }

    async fn verified_path(
        &self,
        object_key: &str,
        expected_sha256_hex: &str,
        expected_size_bytes: u64,
    ) -> Result<PathBuf> {
        let path = self.path_for_key(object_key)?;
        let metadata = self.secure_existing_object_file(&path).await?;
        ensure!(
            metadata.len() == expected_size_bytes,
            "object size mismatch"
        );
        ensure!(
            sha256_file_hex(&path).await? == expected_sha256_hex,
            "object hash mismatch"
        );
        Ok(path)
    }

    async fn ensure_private_object_parent(&self, path: &Path) -> Result<()> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        ensure_private_dir_tree_async(self.root.as_ref(), parent)
            .await
            .with_context(|| format!("failed to create object parent {}", parent.display()))
    }

    async fn secure_existing_object_file(&self, path: &Path) -> Result<std::fs::Metadata> {
        self.ensure_private_object_parent(path).await?;
        let metadata = tokio::fs::symlink_metadata(path)
            .await
            .with_context(|| format!("failed to stat object {}", path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink() && metadata.is_file(),
            "object is not a regular file"
        );
        repair_private_file_permissions_async(path)
            .await
            .with_context(|| format!("failed to secure object {}", path.display()))?;
        tokio::fs::metadata(path)
            .await
            .with_context(|| format!("failed to stat object {}", path.display()))
    }

    async fn delete_best_effort(&self, object_key: &str) {
        if let Ok(path) = self.path_for_key(object_key) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    async fn delete_confirmed(&self, object_key: &str) -> Result<()> {
        let path = self.path_for_key(object_key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("failed to delete object {}", path.display()))
            }
        }
    }

    fn path_for_key(&self, object_key: &str) -> Result<PathBuf> {
        validate_object_key(object_key)?;
        let mut path = (*self.root).clone();
        for segment in object_key.split('/') {
            path.push(segment);
        }
        ensure!(
            path.starts_with(self.root.as_ref()),
            "object key escaped object store root"
        );
        Ok(path)
    }
}

#[derive(Clone, Debug)]
pub struct S3BackupObjectStoreSettings {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub create_bucket: bool,
}

#[derive(Clone, Debug)]
pub struct S3BackupObjectStore {
    endpoint: S3Endpoint,
    bucket: String,
    access_key: Arc<String>,
    secret_key: Arc<String>,
    region: String,
    create_bucket: bool,
}

impl S3BackupObjectStore {
    fn new(settings: S3BackupObjectStoreSettings) -> Result<Self> {
        ensure_s3_bucket_name(&settings.bucket)?;
        ensure!(
            !settings.access_key.trim().is_empty(),
            "S3 access key is required"
        );
        ensure!(
            !settings.secret_key.trim().is_empty(),
            "S3 secret key is required"
        );
        ensure!(!settings.region.trim().is_empty(), "S3 region is required");
        Ok(Self {
            endpoint: S3Endpoint::parse(&settings.endpoint)?,
            bucket: settings.bucket,
            access_key: Arc::new(settings.access_key),
            secret_key: Arc::new(settings.secret_key),
            region: settings.region,
            create_bucket: settings.create_bucket,
        })
    }

    async fn put_new(&self, object_key: &str, bytes: &[u8]) -> Result<()> {
        validate_object_key(object_key)?;
        if self.create_bucket {
            self.ensure_bucket().await?;
        }
        ensure!(
            !self.object_exists(object_key).await?,
            "object already exists"
        );
        let response = self
            .send_signed_request("PUT", Some(object_key), bytes)
            .await?;
        ensure!(
            matches!(response.status_code, 200 | 201),
            "S3 put object failed with HTTP {}: {}",
            response.status_code,
            response.body_text()
        );
        Ok(())
    }

    async fn get(&self, object_key: &str) -> Result<Vec<u8>> {
        self.get_with_limit(object_key, MAX_S3_RESPONSE_BODY_BYTES)
            .await
    }

    async fn get_with_limit(&self, object_key: &str, max_bytes: usize) -> Result<Vec<u8>> {
        validate_object_key(object_key)?;
        let response = self
            .send_signed_request_with_limit("GET", Some(object_key), &[], max_bytes)
            .await?;
        ensure!(
            response.status_code == 200,
            "S3 get object failed with HTTP {}: {}",
            response.status_code,
            response.body_text()
        );
        Ok(response.body)
    }

    async fn spool_verified_to_temp_file(
        &self,
        object_key: &str,
        expected_sha256_hex: &str,
        expected_size_bytes: u64,
        max_bytes: usize,
    ) -> Result<VerifiedObjectFile> {
        validate_object_key(object_key)?;
        ensure!(
            expected_size_bytes <= max_bytes as u64,
            "S3 object exceeded {max_bytes} bytes"
        );
        let mut response = self
            .send_signed_reqwest_request("GET", Some(object_key), &[])
            .await?;
        let status_code = response.status().as_u16();
        if status_code != 200 {
            let body = response
                .bytes()
                .await
                .map(|body| String::from_utf8_lossy(&body).trim().to_string())
                .unwrap_or_else(|error| format!("failed to read error body: {error}"));
            bail!("S3 get object failed with HTTP {status_code}: {body}");
        }
        if let Some(content_length) = response.content_length() {
            ensure!(
                content_length <= max_bytes as u64,
                "S3 response body exceeded {max_bytes} bytes"
            );
            ensure!(
                content_length == expected_size_bytes,
                "S3 object size mismatch"
            );
        }

        let spool_root = std::env::temp_dir().join(S3_SPOOL_DIR);
        ensure_private_dir_async(&spool_root)
            .await
            .with_context(|| {
                format!(
                    "failed to create S3 spool directory {}",
                    spool_root.display()
                )
            })?;
        let temp_path = spool_root.join(format!("{}.part", Uuid::new_v4()));
        let mut file = create_private_file_new_async(&temp_path)
            .await
            .with_context(|| format!("failed to create S3 spool file {}", temp_path.display()))?;
        let mut hasher = Sha256::new();
        let mut written = 0_u64;
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(error).context("failed to read S3 response");
                }
            };
            written = written
                .checked_add(chunk.len() as u64)
                .context("S3 response body size overflow")?;
            if written > max_bytes as u64 {
                let _ = tokio::fs::remove_file(&temp_path).await;
                bail!("S3 response body exceeded {max_bytes} bytes");
            }
            if written > expected_size_bytes {
                let _ = tokio::fs::remove_file(&temp_path).await;
                bail!("S3 object size mismatch");
            }
            hasher.update(&chunk);
            if let Err(error) = file.write_all(&chunk).await {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error).with_context(|| {
                    format!("failed to write S3 spool file {}", temp_path.display())
                });
            }
        }
        if written != expected_size_bytes {
            let _ = tokio::fs::remove_file(&temp_path).await;
            bail!("S3 object size mismatch");
        }
        if hex::encode(hasher.finalize()) != expected_sha256_hex {
            let _ = tokio::fs::remove_file(&temp_path).await;
            bail!("S3 object hash mismatch");
        }
        if let Err(error) = file.sync_data().await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error)
                .with_context(|| format!("failed to sync S3 spool file {}", temp_path.display()));
        }
        Ok(VerifiedObjectFile {
            path: temp_path,
            cleanup_after_stream: true,
        })
    }

    async fn delete_best_effort(&self, object_key: &str) {
        if validate_object_key(object_key).is_ok() {
            let _ = self
                .send_signed_request("DELETE", Some(object_key), &[])
                .await;
        }
    }

    async fn delete_confirmed(&self, object_key: &str) -> Result<()> {
        validate_object_key(object_key)?;
        let response = self
            .send_signed_request("DELETE", Some(object_key), &[])
            .await?;
        ensure!(
            matches!(response.status_code, 200 | 202 | 204 | 404),
            "S3 delete object failed with HTTP {}: {}",
            response.status_code,
            response.body_text()
        );
        Ok(())
    }

    async fn ensure_bucket(&self) -> Result<()> {
        let response = self.send_signed_request("PUT", None, &[]).await?;
        ensure!(
            matches!(response.status_code, 200 | 409),
            "S3 bucket create failed with HTTP {}: {}",
            response.status_code,
            response.body_text()
        );
        Ok(())
    }

    async fn object_exists(&self, object_key: &str) -> Result<bool> {
        let response = self
            .send_signed_request("HEAD", Some(object_key), &[])
            .await?;
        match response.status_code {
            200 => Ok(true),
            404 => Ok(false),
            status => bail!(
                "S3 head object failed with HTTP {status}: {}",
                response.body_text()
            ),
        }
    }

    async fn send_signed_request(
        &self,
        method: &str,
        object_key: Option<&str>,
        body: &[u8],
    ) -> Result<S3HttpResponse> {
        self.send_signed_request_with_limit(method, object_key, body, MAX_S3_RESPONSE_BODY_BYTES)
            .await
    }

    async fn send_signed_request_with_limit(
        &self,
        method: &str,
        object_key: Option<&str>,
        body: &[u8],
        max_response_body_bytes: usize,
    ) -> Result<S3HttpResponse> {
        let response = self
            .send_signed_reqwest_request(method, object_key, body)
            .await?;
        let status_code = response.status().as_u16();
        let content_length = response.content_length();
        ensure!(
            content_length.is_none_or(|length| length <= max_response_body_bytes as u64),
            "S3 response body exceeded {max_response_body_bytes} bytes"
        );
        let body = response
            .bytes()
            .await
            .context("failed to read S3 response")?;
        ensure!(
            body.len() <= max_response_body_bytes,
            "S3 response body exceeded {max_response_body_bytes} bytes"
        );
        Ok(S3HttpResponse {
            status_code,
            body: body.to_vec(),
        })
    }

    async fn send_signed_reqwest_request(
        &self,
        method: &str,
        object_key: Option<&str>,
        body: &[u8],
    ) -> Result<reqwest::Response> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before unix epoch")?
            .as_secs();
        let (date_stamp, amz_date) = amz_dates(now);
        let payload_sha256 = sha256_hex(body);
        let canonical_uri = self.endpoint.canonical_uri(&self.bucket, object_key);
        let authorization = self.authorization_header(
            method,
            &canonical_uri,
            &payload_sha256,
            &date_stamp,
            &amz_date,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-amz-content-sha256"),
            HeaderValue::from_str(&payload_sha256).context("invalid S3 payload hash header")?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-date"),
            HeaderValue::from_str(&amz_date).context("invalid S3 date header")?,
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&authorization).context("invalid S3 authorization header")?,
        );
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_str(&format!("vpsman-api/{}", vpsman_common::release_version()))
                .context("invalid S3 user-agent header")?,
        );
        let client = reqwest::Client::builder()
            .timeout(S3_HTTP_TIMEOUT)
            .build()
            .context("failed to build S3 HTTP client")?;
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).context("invalid S3 HTTP method")?;
        client
            .request(method, self.endpoint.url(&self.bucket, object_key))
            .headers(headers)
            .body(body.to_vec())
            .send()
            .await
            .context("S3 request failed")
    }

    fn authorization_header(
        &self,
        method: &str,
        canonical_uri: &str,
        payload_sha256: &str,
        date_stamp: &str,
        amz_date: &str,
    ) -> String {
        let credential_scope = format!("{date_stamp}/{}/s3/aws4_request", self.region);
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{payload_sha256}\nx-amz-date:{amz_date}\n",
            self.endpoint.host_header(),
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}"
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        let signing_key = aws_signing_key(&self.secret_key, date_stamp, &self.region);
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        )
    }
}

#[derive(Clone, Debug)]
struct S3Endpoint {
    scheme: String,
    host: String,
    port: u16,
    prefix: String,
}

impl S3Endpoint {
    fn parse(endpoint: &str) -> Result<Self> {
        let endpoint = endpoint.trim();
        let (scheme, rest, default_port) = if let Some(rest) = endpoint.strip_prefix("https://") {
            ("https", rest, 443)
        } else if let Some(rest) = endpoint.strip_prefix("http://") {
            ("http", rest, 80)
        } else {
            anyhow::bail!("S3 endpoint must use http:// or https://");
        };
        ensure!(
            !rest.contains('?') && !rest.contains('#'),
            "S3 endpoint is invalid"
        );
        let (authority, raw_path) = rest.split_once('/').unwrap_or((rest, ""));
        ensure!(
            !authority.is_empty() && !authority.contains('@'),
            "S3 endpoint authority is invalid"
        );
        let (host, port) = parse_http_authority(authority, default_port)?;
        ensure!(
            scheme == "https" || is_loopback_s3_host(&host),
            "S3 http:// endpoints are allowed only for localhost or loopback addresses"
        );
        let prefix = normalize_endpoint_prefix(raw_path)?;
        Ok(Self {
            scheme: scheme.to_string(),
            host,
            port,
            prefix,
        })
    }

    fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if (self.scheme == "http" && self.port == 80)
            || (self.scheme == "https" && self.port == 443)
        {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    fn canonical_uri(&self, bucket: &str, object_key: Option<&str>) -> String {
        let mut uri = self.prefix.clone();
        uri.push('/');
        uri.push_str(&percent_encode_segment(bucket));
        if let Some(object_key) = object_key {
            uri.push('/');
            uri.push_str(&percent_encode_path(object_key));
        }
        uri
    }

    fn url(&self, bucket: &str, object_key: Option<&str>) -> String {
        format!(
            "{}://{}{}",
            self.scheme,
            self.host_header(),
            self.canonical_uri(bucket, object_key)
        )
    }
}

fn is_loopback_s3_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false)
}

#[derive(Debug)]
struct S3HttpResponse {
    status_code: u16,
    body: Vec<u8>,
}

impl S3HttpResponse {
    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }
}

pub fn validate_object_key(object_key: &str) -> Result<()> {
    ensure!(!object_key.trim().is_empty(), "object key is required");
    ensure!(
        object_key.len() <= 1024 && !object_key.as_bytes().contains(&0),
        "object key is invalid"
    );
    ensure!(
        !object_key.starts_with('/') && !object_key.contains('\\'),
        "object key must be relative"
    );
    ensure!(
        object_key
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
        "object key contains unsafe path segment"
    );
    Ok(())
}

fn ensure_s3_bucket_name(bucket: &str) -> Result<()> {
    ensure!(
        (3..=63).contains(&bucket.len()),
        "S3 bucket name length is invalid"
    );
    ensure!(
        bucket
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "S3 bucket name must use lowercase letters, digits, or hyphens"
    );
    ensure!(
        !bucket.starts_with('-') && !bucket.ends_with('-'),
        "S3 bucket name cannot start or end with hyphen"
    );
    Ok(())
}

fn parse_http_authority(authority: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .context("invalid bracketed S3 endpoint host")?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .context("invalid S3 endpoint port")?
                .parse::<u16>()
                .context("invalid S3 endpoint port")?
        };
        ensure!(
            port != 0 && !host.is_empty(),
            "S3 endpoint authority is invalid"
        );
        return Ok((host.to_string(), port));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host,
            port.parse::<u16>().context("invalid S3 endpoint port")?,
        ),
        _ => (authority, default_port),
    };
    ensure!(
        port != 0 && !host.is_empty(),
        "S3 endpoint authority is invalid"
    );
    Ok((host.to_string(), port))
}

fn normalize_endpoint_prefix(raw_path: &str) -> Result<String> {
    let raw_path = raw_path.trim_matches('/');
    if raw_path.is_empty() {
        return Ok(String::new());
    }
    ensure!(
        raw_path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
        "S3 endpoint path prefix is invalid"
    );
    Ok(format!("/{}", raw_path.trim_end_matches('/')))
}

fn aws_signing_key(secret_key: &str, date_stamp: &str, region: &str) -> Vec<u8> {
    let date_key = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, b"s3");
    hmac_sha256(&service_key, b"aws4_request")
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts arbitrary key sizes");
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

async fn sha256_file_hex(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read {} for hashing", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn amz_dates(unix_secs: u64) -> (String, String) {
    let days = (unix_secs / 86_400) as i64;
    let seconds_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    (
        format!("{year:04}{month:02}{day:02}"),
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
    )
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u64, u64) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    (year, month as u64, day as u64)
}

fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn filesystem_object_store_writes_private_files_and_dirs() {
        let root =
            std::env::temp_dir().join(format!("vpsman-object-store-private-{}", Uuid::new_v4()));
        let store = FilesystemBackupObjectStore::new(root.clone()).unwrap();

        store
            .put_new("backups/client-a/direct.tar", b"direct")
            .await
            .unwrap();

        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&root.join("backups")), 0o700);
        assert_eq!(mode(&root.join("backups/client-a")), 0o700);
        assert_eq!(mode(&root.join("backups/client-a/direct.tar")), 0o600);

        let source = std::env::temp_dir().join(format!("vpsman-object-source-{}", Uuid::new_v4()));
        tokio::fs::write(&source, b"from-file").await.unwrap();
        let expected_hash = sha256_hex(b"from-file");
        store
            .put_file_idempotent(
                "backups/client-a/from-file.tar",
                &source,
                &expected_hash,
                b"from-file".len() as u64,
            )
            .await
            .unwrap();

        assert_eq!(mode(&root.join("backups/client-a/from-file.tar")), 0o600);
        let _ = tokio::fs::remove_file(source).await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}
