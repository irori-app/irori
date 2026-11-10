//! Bounded reads of the declared hash-tree section, independent of filesystem.
use std::io::{self, ErrorKind, SeekFrom};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};

const WINDOW: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct HashCache {
    start: u64,
    data: Vec<u8>,
}

impl HashCache {
    pub(super) async fn read<'a>(
        &'a mut self,
        reader: &mut (impl AsyncRead + AsyncSeek + Unpin),
        offset: u64,
        length: usize,
        end: u64,
    ) -> io::Result<&'a [u8]> {
        let invalid = || {
            io::Error::new(
                ErrorKind::InvalidData,
                "Hash run exceeds the metadata section.",
            )
        };
        let requested_end = offset.checked_add(length as u64).ok_or_else(invalid)?;
        if requested_end > end || length > WINDOW {
            return Err(invalid());
        }
        if offset < self.start || requested_end > self.start + self.data.len() as u64 {
            self.start = offset;
            let mut data = std::mem::take(&mut self.data);
            data.resize((end - offset).min(WINDOW as u64) as usize, 0);
            reader.seek(SeekFrom::Start(offset)).await?;
            // Do not leave an apparently valid cache after a failed or cancelled fill.
            reader.read_exact(&mut data).await?;
            self.data = data;
        }
        let start = (offset - self.start) as usize;
        Ok(&self.data[start..start + length])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    struct Counting {
        inner: Cursor<Vec<u8>>,
        reads: usize,
        seeks: usize,
        bytes: usize,
    }
    impl AsyncRead for Counting {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.reads += 1;
            let before = buf.filled().len();
            let result = Pin::new(&mut self.inner).poll_read(cx, buf);
            self.bytes += buf.filled().len() - before;
            result
        }
    }
    impl AsyncSeek for Counting {
        fn start_seek(mut self: Pin<&mut Self>, pos: SeekFrom) -> io::Result<()> {
            self.seeks += 1;
            Pin::new(&mut self.inner).start_seek(pos)
        }
        fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
            Pin::new(&mut self.inner).poll_complete(cx)
        }
    }
    fn fixture(length: usize) -> Counting {
        Counting {
            inner: Cursor::new((0..length).map(|n| (n % 251) as u8).collect()),
            reads: 0,
            seeks: 0,
            bytes: 0,
        }
    }

    #[tokio::test]
    async fn streaming_hash_cache_full_xvd_parser_preserves_every_hash_and_encryption_unit() {
        use crate::parse::{BinaryParse, BinaryTryParse};
        use crate::{
            layout::{PAGE_SIZE, Pages},
            math::calculate_hash_block_num_and_run_for_block_num,
            models::xvd::{XvdHashEntry, XvdHeader},
            xvd::XvdFile,
        };
        let count = 50000u32;
        let mut header = XvdHeader::buffer();
        header[512..520].copy_from_slice(b"msft-xvd");
        header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
        header[536..544].copy_from_slice(&((count as u64 + 20) * PAGE_SIZE as u64).to_le_bytes());
        header[656..660].copy_from_slice(&4096u32.to_le_bytes());
        let parsed_header = XvdHeader::try_from_array(&header).unwrap();
        let layout = parsed_header.layout();
        let mut data = vec![0; layout.xvc_info.start.to_bytes().0 as usize + 4096];
        data[..header.len()].copy_from_slice(&header);
        let info = layout.xvc_info.start.to_bytes().0 as usize;
        data[info + 3344..info + 3348].copy_from_slice(&1u32.to_le_bytes());
        data[info + 3348..info + 3352].copy_from_slice(&1u32.to_le_bytes());
        data[info + 3376..info + 3384].copy_from_slice(&116444736000000000u64.to_le_bytes());
        let region = info + 3496;
        let start = Pages(layout.drive_data.start.0 + 7);
        data[region + 80..region + 88].copy_from_slice(&start.to_bytes().0.to_le_bytes());
        data[region + 88..region + 96].copy_from_slice(&Pages(count).to_bytes().0.to_le_bytes());
        for page in 0..count {
            let (block, entry, _) = calculate_hash_block_num_and_run_for_block_num(
                0,
                layout.hash_tree_layout.top_level() as u64 + 1,
                parsed_header.number_of_hashed_pages(),
                start.0 - layout.user_data.start.0 + page,
                0,
                false,
                false,
            );
            let offset = (layout.hash_tree.start.to_bytes().0 + Pages(block).to_bytes().0) as usize
                + entry as usize * XvdHashEntry::SIZE;
            data[offset..offset + 20].fill((page % 251) as u8);
            data[offset + 20..offset + 24].copy_from_slice(&(page * 7).to_le_bytes());
        }
        let mut reader = Counting {
            inner: Cursor::new(data),
            reads: 0,
            seeks: 0,
            bytes: 0,
        };
        let parsed = XvdFile::parse(&mut reader).await.unwrap();
        assert_eq!(parsed.encrypted_section_infos.len(), 1);
        let section = &parsed.encrypted_section_infos[0];
        assert_eq!(section.data_hashs.len(), count as usize);
        for page in 0..count {
            assert_eq!(section.data_hashs[page as usize], [(page % 251) as u8; 20]);
            assert_eq!(
                section.data_units.as_ref().unwrap()[page as usize],
                page * 7
            );
        }
        assert!(
            reader.reads < 10,
            "full parser should batch hash-tree reads, got {}",
            reader.reads
        );
    }

    #[tokio::test]
    async fn streaming_hash_cache_batches_strided_reads_and_preserves_backward_and_boundary_bytes()
    {
        let mut reader = fixture(3 * WINDOW + 17);
        let mut cache = HashCache::default();
        let end = reader.inner.get_ref().len() as u64;
        for n in 0..500 {
            let start = n * 4096;
            let bytes = cache
                .read(&mut reader, start as u64, 4080, end)
                .await
                .unwrap();
            assert_eq!(
                bytes,
                (start..start + 4080)
                    .map(|n| (n % 251) as u8)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            reader.reads, 2,
            "one read per MiB rather than one per hash block"
        );
        assert_eq!(reader.seeks, 2);
        for (start, length) in [(12, 31), (WINDOW - 12, 48), (3 * WINDOW, 17)] {
            assert_eq!(
                cache
                    .read(&mut reader, start as u64, length, end)
                    .await
                    .unwrap(),
                (start..start + length)
                    .map(|n| (n % 251) as u8)
                    .collect::<Vec<_>>()
            );
        }
        assert!(cache.data.len() <= WINDOW);
    }

    #[tokio::test]
    async fn streaming_hash_cache_never_reads_beyond_declared_section_or_accepts_truncated_data() {
        let mut reader = fixture(100);
        let mut cache = HashCache::default();
        assert_eq!(cache.read(&mut reader, 10, 20, 37).await.unwrap().len(), 20);
        assert_eq!(reader.bytes, 27);
        assert!(cache.read(&mut reader, 38, 0, 37).await.is_err());
        assert!(
            cache
                .read(&mut reader, u64::MAX, 2, u64::MAX)
                .await
                .is_err()
        );
        assert!(
            cache
                .read(&mut reader, 0, WINDOW + 1, 2 * WINDOW as u64)
                .await
                .is_err()
        );
        assert!(cache.read(&mut reader, 90, 20, 110).await.is_err());
        assert!(cache.data.is_empty());
        assert_eq!(
            cache.read(&mut reader, 90, 10, 100).await.unwrap(),
            (90..100).collect::<Vec<u8>>()
        );
    }

    struct PartialThenPending(bool);
    impl AsyncRead for PartialThenPending {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.0 {
                return Poll::Pending;
            }
            self.0 = true;
            buf.put_slice(&[1; 11]);
            Poll::Ready(Ok(()))
        }
    }
    impl AsyncSeek for PartialThenPending {
        fn start_seek(self: Pin<&mut Self>, _: SeekFrom) -> io::Result<()> {
            Ok(())
        }
        fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }
    #[tokio::test]
    async fn streaming_hash_cache_cancelled_fill_is_never_reused() {
        let mut cache = HashCache::default();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                cache.read(&mut PartialThenPending(false), 0, 20, 100)
            )
            .await
            .is_err()
        );
        assert!(cache.data.is_empty());
        assert_eq!(
            cache.read(&mut fixture(100), 0, 20, 100).await.unwrap(),
            (0..20).collect::<Vec<u8>>()
        );
    }
}
