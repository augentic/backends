use std::fmt::Debug;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use azure_core::http::RequestContent;
use azure_storage_blob::BlobServiceClient;
use azure_storage_blob::models::{
    BlobClientDownloadOptions, BlobClientGetPropertiesResultHeaders,
    BlobContainerClientCreateOptions, BlobContainerClientDeleteOptions,
};
use futures::{FutureExt, TryStreamExt};
use omnia_wasi_blobstore::{
    Container, ContainerMetadata, FutureResult, ObjectMetadata, WasiBlobstoreCtx,
};

use crate::Client;

fn now_unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Build download options with an HTTP `Range` header for partial reads.
///
/// Returns `Ok(None)` for full-object reads (the convention is `start=0, end=0`
/// or `start=0, end=u64::MAX`). When `end` is `0` or `u64::MAX` but `start > 0`,
/// an open-ended range (`bytes=start-`) is used.
///
/// Returns `Err` when `end` is a concrete (non-sentinel) value that is less than
/// `start`, which would produce an invalid HTTP Range header.
fn range_options(
    start: u64, end: u64,
) -> anyhow::Result<Option<BlobClientDownloadOptions<'static>>> {
    if start == 0 && (end == 0 || end == u64::MAX) {
        return Ok(None);
    }

    let unbounded = end == 0 || end == u64::MAX;

    if !unbounded && end < start {
        anyhow::bail!("invalid byte range: end ({end}) < start ({start})");
    }

    let start = usize::try_from(start).context("start offset out of range")?;
    let range = if unbounded {
        start..usize::MAX
    } else {
        let end = usize::try_from(end).context("end offset out of range")?;
        let exclusive_end = end.saturating_add(1);
        start..exclusive_end
    };

    Ok(Some(BlobClientDownloadOptions {
        range: Some(range),
        ..Default::default()
    }))
}

/// `wasi-blobstore` implementation backed by Azure Blob Storage.
impl WasiBlobstoreCtx for Client {
    fn create_container(&self, name: String) -> FutureResult<Arc<dyn Container>> {
        tracing::trace!("creating container: {name}");
        let service = Arc::clone(&self.service);

        async move {
            let container_client = service.blob_container_client(&name);
            container_client
                .create(Option::<BlobContainerClientCreateOptions<'_>>::None)
                .await
                .context("creating container")?;

            let created_at = now_unix_secs();

            Ok(Arc::new(AzureBlobContainer {
                name,
                service,
                created_at,
            }) as Arc<dyn Container>)
        }
        .boxed()
    }

    fn get_container(&self, name: String) -> FutureResult<Arc<dyn Container>> {
        tracing::trace!("getting container: {name}");
        let service = Arc::clone(&self.service);

        async move {
            Ok(Arc::new(AzureBlobContainer {
                name,
                service,
                created_at: 0,
            }) as Arc<dyn Container>)
        }
        .boxed()
    }

    fn delete_container(&self, name: String) -> FutureResult<()> {
        tracing::trace!("deleting container: {name}");
        let service = Arc::clone(&self.service);

        async move {
            service
                .blob_container_client(&name)
                .delete(Option::<BlobContainerClientDeleteOptions<'_>>::None)
                .await
                .context("deleting container")?;
            Ok(())
        }
        .boxed()
    }

    fn container_exists(&self, name: String) -> FutureResult<bool> {
        tracing::trace!("checking existence of container: {name}");
        let service = Arc::clone(&self.service);

        async move {
            service
                .blob_container_client(&name)
                .exists()
                .await
                .context("checking container existence")
        }
        .boxed()
    }
}

/// A blobstore container backed by an Azure Blob Storage container.
struct AzureBlobContainer {
    name: String,
    service: Arc<BlobServiceClient>,
    created_at: u64,
}

impl Debug for AzureBlobContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureBlobContainer").field("name", &self.name).finish_non_exhaustive()
    }
}

impl Container for AzureBlobContainer {
    fn name(&self) -> anyhow::Result<String> {
        tracing::trace!("getting container name");
        Ok(self.name.clone())
    }

    fn info(&self) -> anyhow::Result<ContainerMetadata> {
        tracing::trace!("getting container info");
        Ok(ContainerMetadata {
            name: self.name.clone(),
            created_at: self.created_at,
        })
    }

    fn get_data(&self, name: String, start: u64, end: u64) -> FutureResult<Option<Vec<u8>>> {
        tracing::trace!("getting object data: {name}");
        let blob_client = self.service.blob_client(&self.name, &name);

        async move {
            let response = blob_client
                .download(range_options(start, end)?)
                .await
                .context("downloading blob")?;
            let data: Vec<u8> =
                response.body.collect().await.context("reading blob body")?.to_vec();
            Ok(Some(data))
        }
        .boxed()
    }

    fn write_data(&self, name: String, data: Vec<u8>) -> FutureResult<()> {
        tracing::trace!("writing object data: {name}");
        let blob_client = self.service.blob_client(&self.name, &name);

        async move {
            let content = RequestContent::from(data);
            blob_client.upload(content, None).await.context("uploading blob")?;
            Ok(())
        }
        .boxed()
    }

    fn list_objects(&self) -> FutureResult<Vec<String>> {
        tracing::trace!("listing objects");
        let container_client = self.service.blob_container_client(&self.name);

        async move {
            let pager = container_client.list_blobs(None).context("listing blobs")?;

            let items: Vec<_> = pager.try_collect().await.context("paginating blob list")?;
            let names = items.into_iter().filter_map(|item| item.name).collect();

            Ok(names)
        }
        .boxed()
    }

    fn delete_object(&self, name: String) -> FutureResult<()> {
        tracing::trace!("deleting object: {name}");
        let blob_client = self.service.blob_client(&self.name, &name);

        async move {
            blob_client.delete(None).await.context("deleting blob")?;
            Ok(())
        }
        .boxed()
    }

    fn has_object(&self, name: String) -> FutureResult<bool> {
        tracing::trace!("checking existence of object: {name}");
        let blob_client = self.service.blob_client(&self.name, &name);

        async move { blob_client.exists().await.context("checking blob existence") }.boxed()
    }

    fn object_info(&self, name: String) -> FutureResult<ObjectMetadata> {
        tracing::trace!("getting object info: {name}");
        let blob_client = self.service.blob_client(&self.name, &name);
        let container_name = self.name.clone();

        async move {
            let response =
                blob_client.get_properties(None).await.context("getting blob properties")?;

            let size = response.content_length().ok().flatten().unwrap_or(0);

            let created_at = response
                .creation_time()
                .ok()
                .flatten()
                .map_or(0, |t| u64::try_from(t.unix_timestamp()).unwrap_or(0));

            Ok(ObjectMetadata {
                name,
                container: container_name,
                size,
                created_at,
            })
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use azure_storage_blob::models::{BlobItem, BlobProperties};

    use super::*;

    fn collect_blob_names(items: Vec<BlobItem>) -> Vec<String> {
        items.into_iter().filter_map(|item| item.name).collect()
    }

    fn object_metadata_from_properties(
        name: String, container: String, props: &BlobProperties,
    ) -> ObjectMetadata {
        let size = props.content_length.unwrap_or(0);

        let created_at =
            props.creation_time.map_or(0, |t| u64::try_from(t.unix_timestamp()).unwrap_or(0));

        ObjectMetadata {
            name,
            container,
            size,
            created_at,
        }
    }

    fn blob_item(name: Option<&str>) -> BlobItem {
        let mut item = BlobItem::default();
        item.name = name.map(String::from);
        item
    }

    fn blob_props(content_length: Option<u64>, unix_secs: Option<i64>) -> BlobProperties {
        let mut props = BlobProperties::default();
        props.content_length = content_length;
        props.creation_time =
            unix_secs.map(|s| azure_core::time::OffsetDateTime::from_unix_timestamp(s).unwrap());
        props
    }

    #[test]
    fn collect_names_from_blob_items() {
        let items =
            vec![blob_item(Some("file1.txt")), blob_item(Some("dir/file2.json")), blob_item(None)];

        let names = collect_blob_names(items);
        assert_eq!(names, vec!["file1.txt", "dir/file2.json"]);
    }

    #[test]
    fn collect_names_empty_list() {
        let names = collect_blob_names(vec![]);
        assert!(names.is_empty());
    }

    #[test]
    fn metadata_from_properties_with_values() {
        let props = blob_props(Some(1024), Some(1_700_000_000));

        let meta = object_metadata_from_properties("blob.bin".into(), "mycontainer".into(), &props);

        assert_eq!(meta.name, "blob.bin");
        assert_eq!(meta.container, "mycontainer");
        assert_eq!(meta.size, 1024);
        assert_eq!(meta.created_at, 1_700_000_000);
    }

    #[test]
    fn metadata_from_properties_defaults_when_none() {
        let props = blob_props(None, None);

        let meta = object_metadata_from_properties("empty.txt".into(), "c".into(), &props);

        assert_eq!(meta.name, "empty.txt");
        assert_eq!(meta.container, "c");
        assert_eq!(meta.size, 0);
        assert_eq!(meta.created_at, 0);
    }

    #[test]
    fn metadata_from_properties_large_blob() {
        let props = blob_props(Some(5_368_709_120), Some(1_000_000_000));

        let meta = object_metadata_from_properties("large.zip".into(), "backups".into(), &props);

        assert_eq!(meta.size, 5_368_709_120);
        assert_eq!(meta.created_at, 1_000_000_000);
    }

    #[test]
    fn metadata_from_properties_negative_timestamp_defaults_to_zero() {
        let props = blob_props(Some(256), Some(-86_400));

        let meta = object_metadata_from_properties("old.dat".into(), "archive".into(), &props);

        assert_eq!(meta.created_at, 0);
    }

    #[test]
    fn range_options_full_read_zero_zero() {
        assert!(range_options(0, 0).unwrap().is_none());
    }

    #[test]
    fn range_options_full_read_zero_max() {
        assert!(range_options(0, u64::MAX).unwrap().is_none());
    }

    #[test]
    fn range_options_offset_with_unbounded_end() {
        let opts = range_options(100, u64::MAX).unwrap().expect("should produce options");
        assert_eq!(opts.range, Some(100..usize::MAX));
    }

    #[test]
    fn range_options_offset_with_zero_end() {
        let opts = range_options(100, 0).unwrap().expect("should produce options");
        assert_eq!(opts.range, Some(100..usize::MAX));
    }

    #[test]
    fn range_options_bounded_range() {
        let opts = range_options(10, 99).unwrap().expect("should produce options");
        assert_eq!(opts.range, Some(10..100));
    }

    #[test]
    fn range_options_single_byte() {
        let opts = range_options(5, 5).unwrap().expect("should produce options");
        assert_eq!(opts.range, Some(5..6));
    }

    #[test]
    fn range_options_rejects_end_less_than_start() {
        let err = range_options(10, 5).unwrap_err();
        assert!(err.to_string().contains("end (5) < start (10)"));
    }
}
