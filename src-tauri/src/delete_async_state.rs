//! 删除依赖计数：目录 CLOSE 和全部子项删除确认后，才允许 RMDIR。
use super::*;
use std::{collections::BTreeMap, time::Instant};
pub(super) struct Directory {
    pub path: String,
    pub parent: Option<usize>,
    pub depth: usize,
    pub children: usize,
    pub closed: bool,
    pub removal_queued: bool,
    pub handle: Option<String>,
    pub permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
pub(super) struct Entry {
    pub path: String,
    pub parent: usize,
    pub depth: usize,
    pub listed: Option<russh_sftp::protocol::FileAttributes>,
}
pub(super) enum Scanned {
    Opened(Entry, String, tokio::sync::OwnedSemaphorePermit),
    File(Entry),
    Batch(usize, Vec<(String, russh_sftp::protocol::FileAttributes)>),
    Eof(usize),
    Closed(usize),
}
pub(super) enum Mutation {
    File(String, usize, Instant),
    Directory(String, usize, Instant),
}
pub(super) fn ready(tree: &mut [Directory], id: usize, queue: &mut VecDeque<Mutation>) {
    let directory = &mut tree[id];
    if id != 0 && directory.closed && directory.children == 0 && !directory.removal_queued {
        directory.removal_queued = true;
        // 已完成子树优先收尾，让依赖链继续推进，不排在全部 UNLINK 后。
        queue.push_front(Mutation::Directory(
            directory.path.clone(),
            id,
            Instant::now(),
        ));
    }
}
pub(super) fn completed(
    tree: &mut [Directory],
    parent: usize,
    queue: &mut VecDeque<Mutation>,
) -> Result<()> {
    tree[parent].children = tree[parent]
        .children
        .checked_sub(1)
        .ok_or_else(|| AppError::new("删除依赖计数无效"))?;
    ready(tree, parent, queue);
    Ok(())
}
#[derive(Default)]
struct RequestStats {
    count: usize,
    total_ms: u128,
    max_ms: u128,
    failed: usize,
    first_ms: Option<u128>,
    last_ms: u128,
}
pub(super) struct Trace {
    pub id: String,
    started: Instant,
    stats: std::sync::Mutex<BTreeMap<&'static str, RequestStats>>,
    active: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
}
struct ActiveRequest<'a>(&'a std::sync::atomic::AtomicUsize);
impl Drop for ActiveRequest<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}
impl Trace {
    pub fn new() -> Self {
        Self {
            id: identifier(),
            started: Instant::now(),
            stats: Default::default(),
            active: Default::default(),
            peak: Default::default(),
        }
    }
    pub async fn request<T>(
        &self,
        phase: &'static str,
        future: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let started = Instant::now();
        let active = self
            .active
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.peak
            .fetch_max(active, std::sync::atomic::Ordering::Relaxed);
        let active = ActiveRequest(&self.active);
        let result = future.await;
        drop(active);
        if let Ok(mut stats) = self.stats.lock() {
            let stats = stats.entry(phase).or_default();
            let elapsed = started.elapsed().as_millis();
            let first_ms = started.duration_since(self.started).as_millis();
            stats.first_ms = Some(stats.first_ms.map_or(first_ms, |first| first.min(first_ms)));
            stats.last_ms = self.started.elapsed().as_millis();
            stats.count += 1;
            stats.total_ms += elapsed;
            stats.max_ms = stats.max_ms.max(elapsed);
            stats.failed += usize::from(result.is_err());
        }
        result
    }
    /// 只记录发现项目后到实际派发的本地队列等待；不含协议响应时间。
    pub fn queued(&self, phase: &'static str, queued: Instant) {
        if let Ok(mut stats) = self.stats.lock() {
            let stats = stats.entry(phase).or_default();
            let elapsed = queued.elapsed().as_millis();
            let first_ms = queued.duration_since(self.started).as_millis();
            stats.first_ms = Some(stats.first_ms.map_or(first_ms, |first| first.min(first_ms)));
            stats.last_ms = self.started.elapsed().as_millis();
            stats.count += 1;
            stats.total_ms += elapsed;
            stats.max_ms = stats.max_ms.max(elapsed);
        }
    }
    pub fn tail(&self, queued: usize, active: usize) {
        crate::performance_log!("[TermRill delete] task={} phase=scan_complete elapsed_ms={} mutation_queued={} mutation_active={} mutation_limit=40", self.id, self.started.elapsed().as_millis(), queued, active);
    }
    pub fn finish(&self, success: bool, discovered: usize, scan_peak: usize, mutation_peak: usize) {
        if let Ok(stats) = self.stats.lock() {
            for (phase, stats) in stats.iter() {
                crate::performance_log!("[TermRill delete] task={} phase={} count={} request_total_ms={} max_ms={} failed={} first_ms={} last_ms={}", self.id, phase, stats.count, stats.total_ms, stats.max_ms, stats.failed, stats.first_ms.unwrap_or(0), stats.last_ms);
            }
        }
        // 请求耗时含并发重叠，不能累加成用户等待时间；total 是独立墙钟计时。
        crate::performance_log!("[TermRill delete] task={} engine=async-stream phase=total elapsed_ms={} success={} discovered={} scan_peak={} mutation_peak={} request_peak={}",self.id,self.started.elapsed().as_millis(),success,discovered,scan_peak,mutation_peak,self.peak.load(std::sync::atomic::Ordering::Relaxed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(parent: Option<usize>, children: usize, closed: bool) -> Directory {
        Directory {
            path: "/fixture".into(),
            parent,
            depth: 0,
            children,
            closed,
            removal_queued: false,
            handle: None,
            permit: None,
        }
    }
    #[test]
    fn parent_waits_for_close_and_child_ack_and_is_queued_once() {
        let mut tree = vec![node(None, 1, true), node(Some(0), 1, false)];
        let mut queue = VecDeque::new();
        completed(&mut tree, 1, &mut queue).unwrap();
        assert!(queue.is_empty());
        tree[1].closed = true;
        ready(&mut tree, 1, &mut queue);
        ready(&mut tree, 1, &mut queue);
        assert_eq!(queue.len(), 1);
        assert_eq!(tree[0].children, 1);
        completed(&mut tree, 0, &mut queue).unwrap();
        assert_eq!(tree[0].children, 0);
        assert!(completed(&mut tree, 0, &mut queue).is_err());
    }
    #[test]
    fn completed_branch_does_not_wait_for_unrelated_sibling() {
        let mut tree = vec![
            node(None, 1, true),
            node(Some(0), 2, true),
            node(Some(1), 0, true),
            node(Some(1), 1, false),
        ];
        let mut queue = VecDeque::new();
        ready(&mut tree, 2, &mut queue);
        assert!(matches!(
            queue.pop_front(),
            Some(Mutation::Directory(_, 2, _))
        ));
        completed(&mut tree, 1, &mut queue).unwrap();
        assert!(queue.is_empty());
        assert_eq!(tree[1].children, 1);
    }
}
