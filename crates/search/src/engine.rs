//! Meilisearch 封装：索引名、设置、文档读写、查询。错误一律向上返回，由调用方决定降级。
//!
//! 文档里的 id 一律存成字符串：雪花 id 超过 2^53，Meilisearch 的数值过滤按 f64 比较会丢精度。

use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use ik_kernel::config::Search as SearchConfig;
use meilisearch_sdk::client::Client;
use meilisearch_sdk::documents::{DocumentDeletionQuery, DocumentsQuery};
use meilisearch_sdk::errors::{Error as MeiliError, ErrorCode};
use meilisearch_sdk::indexes::Index;
use meilisearch_sdk::search::Selectors;
use meilisearch_sdk::settings::Settings;
use meilisearch_sdk::task_info::TaskInfo;
use serde::{Deserialize, Serialize};

use crate::text::{MARK_END, MARK_START, filter_string};

/// 文档 id 与作者 id 在索引里的字符串形式。
fn sid(id: i64) -> String {
    id.to_string()
}

/// 帖子文档（SRCH-001）。匿名帖的作者名与作者 id 为空（PRIV-005）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostDocument {
    pub id: String,
    pub title: String,
    pub text: String,
    pub author_name: Option<String>,
    pub author_user_id: Option<String>,
    pub category_name: Option<String>,
    pub category_slug: Option<String>,
    pub views: i64,
    pub likes: i32,
    pub comments: i32,
    /// 信息流时间（毫秒），「最新」按它排序（D-038 方案 B）。
    pub bumped_at: i64,
    /// 同步代次：全量对账结束时删除代次更旧的文档（索引残留）。
    pub generation: i64,
}

/// 评论文档（SRCH-002）。匿名评论的作者名与作者 id 为空。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentDocument {
    pub id: String,
    pub post_id: String,
    pub content: String,
    pub post_title: String,
    pub author_name: Option<String>,
    pub author_user_id: Option<String>,
    pub likes: i32,
    pub created_at: i64,
    pub generation: i64,
}

#[derive(Debug, Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorNamePatch<'a> {
    id: String,
    author_name: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostSort {
    Relevance,
    /// 信息流时间倒序。
    Latest,
    /// 点赞、浏览倒序。
    Hot,
}

/// 一次查询的命中：按引擎顺序的 id 与估计总数。
#[derive(Debug, Default)]
pub(crate) struct Hits {
    pub ids: Vec<i64>,
    pub total: usize,
}

/// 联想命中：标题高亮（带占位符）与正文片段。
#[derive(Debug)]
pub(crate) struct SuggestHit {
    pub id: i64,
    pub title_marked: Option<String>,
    pub excerpt: Option<String>,
}

/// 等设置生效（索引重建）的上限。
const SETTINGS_WAIT: Duration = Duration::from_secs(600);

/// 我们推送的几项设置是否与线上一致。可过滤、可排序字段不讲顺序，其余讲顺序。
fn settings_match(want: &Settings, have: &Settings) -> bool {
    fn unordered<T: Serialize>(list: &Option<Vec<T>>) -> Option<Vec<String>> {
        list.as_ref().map(|items| {
            let mut keys: Vec<String> = items
                .iter()
                .map(|item| serde_json::to_string(item).unwrap_or_default())
                .collect();
            keys.sort();
            keys
        })
    }
    want.searchable_attributes == have.searchable_attributes
        && want.ranking_rules == have.ranking_rules
        && unordered(&want.filterable_attributes) == unordered(&have.filterable_attributes)
        && unordered(&want.sortable_attributes) == unordered(&have.sortable_attributes)
}

pub(crate) struct Engine {
    client: Client,
    posts: Index,
    comments: Index,
    timeout: Duration,
    /// 最近一次提交的写任务（测试与 CLI 等它处理完）。
    last_task: Mutex<Option<TaskInfo>>,
}

async fn within<T>(
    timeout: Duration,
    what: &'static str,
    future: impl Future<Output = Result<T, MeiliError>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(timeout, future)
        .await
        .with_context(|| format!("Meilisearch {what} 超时"))?
        .with_context(|| format!("Meilisearch {what} 失败"))
}

/// `NOT authorUserId IN [...]`：拉黑过滤。匿名文档没有作者 id，不会被过滤（PRIV-001）。
fn exclude_authors(excluded: &[i64]) -> Option<String> {
    if excluded.is_empty() {
        return None;
    }
    let ids: Vec<String> = excluded.iter().map(|id| filter_string(&sid(*id))).collect();
    Some(format!("NOT authorUserId IN [{}]", ids.join(", ")))
}

fn and(parts: Vec<String>) -> Option<String> {
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

fn parse_ids<T>(
    hits: &[meilisearch_sdk::search::SearchResult<T>],
    id: impl Fn(&T) -> &str,
) -> Vec<i64> {
    hits.iter()
        .filter_map(|h| id(&h.result).parse().ok())
        .collect()
}

impl Engine {
    pub(crate) fn new(config: &SearchConfig, url: &str) -> anyhow::Result<Self> {
        let key = (!config.api_key.is_empty()).then(|| config.api_key.expose().to_owned());
        let client = Client::new(url, key).context("创建 Meilisearch 客户端失败")?;
        let prefix = &config.index_prefix;
        Ok(Self {
            posts: client.index(format!("{prefix}post")),
            comments: client.index(format!("{prefix}comment")),
            client,
            timeout: Duration::from_millis(config.timeout_ms),
            last_task: Mutex::new(None),
        })
    }

    fn remember(&self, task: TaskInfo) {
        if let Ok(mut last) = self.last_task.lock() {
            *last = Some(task);
        }
    }

    /// 写入类请求的超时：比查询宽松（批量文档）。
    fn write_timeout(&self) -> Duration {
        self.timeout.max(Duration::from_secs(10))
    }

    /// 建索引（已存在则跳过）并推送设置（SRCH-050：配置是唯一事实来源，每次启动都核对）。
    ///
    /// 同名索引若主键不是 `id`，说明是别的系统的（例如旧站），直接报错，不碰它。
    /// 设置与线上一致就不推：Meilisearch 收到设置就会重建整个索引，几万条文档要几十秒。
    pub(crate) async fn ensure(&self) -> anyhow::Result<()> {
        let wait = Some(Duration::from_secs(30));
        for index in [&self.posts, &self.comments] {
            let task = within(
                self.write_timeout(),
                "建索引",
                self.client.create_index(&index.uid, Some("id")),
            )
            .await?;
            let done = within(
                Duration::from_secs(35),
                "建索引",
                task.wait_for_completion(&self.client, None, wait),
            )
            .await?;
            if done.is_failure() {
                let error = done.unwrap_failure();
                if error.error_code != ErrorCode::IndexAlreadyExists {
                    anyhow::bail!("建索引 {} 失败：{error}", index.uid);
                }
            }
            let existing = within(
                self.write_timeout(),
                "读取索引",
                self.client.get_index(&index.uid),
            )
            .await?;
            if existing.primary_key.as_deref() != Some("id") {
                anyhow::bail!(
                    "索引 {} 的主键是 {:?}，不是本系统建的；请设置 IK_SEARCH__INDEX_PREFIX 换一个索引名",
                    index.uid,
                    existing.primary_key
                );
            }
        }
        let post_settings = Settings::new()
            .with_searchable_attributes(["title", "text", "authorName", "categoryName"])
            .with_filterable_attributes(["categorySlug", "authorUserId", "generation"])
            .with_sortable_attributes(["bumpedAt", "views", "likes"])
            .with_ranking_rules([
                "words",
                "typo",
                "proximity",
                "attribute",
                "sort",
                "exactness",
                "bumpedAt:desc",
            ]);
        let comment_settings = Settings::new()
            .with_searchable_attributes(["content", "postTitle", "authorName"])
            .with_filterable_attributes(["postId", "authorUserId", "generation"])
            .with_sortable_attributes(["createdAt", "likes"])
            .with_ranking_rules([
                "words",
                "typo",
                "proximity",
                "attribute",
                "sort",
                "exactness",
                "createdAt:desc",
            ]);
        for (index, settings) in [
            (&self.posts, post_settings),
            (&self.comments, comment_settings),
        ] {
            let current = within(self.write_timeout(), "读取设置", index.get_settings()).await?;
            if settings_match(&settings, &current) {
                continue;
            }
            tracing::info!(index = %index.uid, "推送搜索索引设置（会重建索引）");
            let task = within(
                self.write_timeout(),
                "推送设置",
                index.set_settings(&settings),
            )
            .await?;
            // 重建整个索引：文档多时要几十秒到几分钟。
            let done = within(
                SETTINGS_WAIT + Duration::from_secs(5),
                "推送设置",
                task.wait_for_completion(&self.client, None, Some(SETTINGS_WAIT)),
            )
            .await?;
            if done.is_failure() {
                anyhow::bail!("推送 {} 设置失败：{}", index.uid, done.unwrap_failure());
            }
        }
        Ok(())
    }

    /// 等最近一次写任务处理完（测试、全量对账收尾用）。
    pub(crate) async fn settle(&self) -> anyhow::Result<()> {
        let task = self.last_task.lock().ok().and_then(|mut t| t.take());
        if let Some(task) = task {
            within(
                Duration::from_secs(60),
                "等待任务",
                self.client
                    .wait_for_task(task, None, Some(Duration::from_secs(55))),
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn upsert_posts(&self, docs: &[PostDocument]) -> anyhow::Result<()> {
        if docs.is_empty() {
            return Ok(());
        }
        let task = within(
            self.write_timeout(),
            "写入帖子文档",
            self.posts.add_or_replace(docs, Some("id")),
        )
        .await?;
        self.remember(task);
        Ok(())
    }

    pub(crate) async fn upsert_comments(&self, docs: &[CommentDocument]) -> anyhow::Result<()> {
        if docs.is_empty() {
            return Ok(());
        }
        let task = within(
            self.write_timeout(),
            "写入评论文档",
            self.comments.add_or_replace(docs, Some("id")),
        )
        .await?;
        self.remember(task);
        Ok(())
    }

    pub(crate) async fn delete_posts(&self, ids: &[i64]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = ids.iter().map(|id| sid(*id)).collect();
        let task = within(
            self.write_timeout(),
            "删除帖子文档",
            self.posts.delete_documents(&ids),
        )
        .await?;
        self.remember(task);
        Ok(())
    }

    pub(crate) async fn delete_comments(&self, ids: &[i64]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = ids.iter().map(|id| sid(*id)).collect();
        let task = within(
            self.write_timeout(),
            "删除评论文档",
            self.comments.delete_documents(&ids),
        )
        .await?;
        self.remember(task);
        Ok(())
    }

    async fn delete_comments_where(&self, filter: &str) -> anyhow::Result<()> {
        let mut query = DocumentDeletionQuery::new(&self.comments);
        query.with_filter(filter);
        let task = within(
            self.write_timeout(),
            "按条件删除评论文档",
            self.comments.delete_documents_with(&query),
        )
        .await?;
        self.remember(task);
        Ok(())
    }

    /// 删除这些帖子下的全部评论文档（帖子下线、删除、隐藏时）。
    pub(crate) async fn delete_comments_of_posts(&self, post_ids: &[i64]) -> anyhow::Result<()> {
        if post_ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = post_ids.iter().map(|id| filter_string(&sid(*id))).collect();
        self.delete_comments_where(&format!("postId IN [{}]", ids.join(", ")))
            .await
    }

    /// 删除代次早于 `generation` 的文档（全量对账收尾：库里已经没有的残留）。
    pub(crate) async fn delete_older_than(&self, generation: i64) -> anyhow::Result<()> {
        let filter = format!("generation < {generation}");
        let mut query = DocumentDeletionQuery::new(&self.posts);
        query.with_filter(&filter);
        let task = within(
            self.write_timeout(),
            "清理残留帖子文档",
            self.posts.delete_documents_with(&query),
        )
        .await?;
        self.remember(task);
        self.delete_comments_where(&filter).await
    }

    /// 把某人非匿名文档的作者名改成 `name`（改名、注销，SRCH-051、D-039）。
    pub(crate) async fn rename_author(&self, user_id: i64, name: &str) -> anyhow::Result<()> {
        const PAGE: usize = 1000;
        let filter = format!("authorUserId = {}", filter_string(&sid(user_id)));
        for index in [&self.posts, &self.comments] {
            let mut offset = 0;
            loop {
                let mut query = DocumentsQuery::new(index);
                query
                    .with_filter(&filter)
                    .with_fields(["id"])
                    .with_offset(offset)
                    .with_limit(PAGE);
                let page = within(
                    self.write_timeout(),
                    "按作者取文档",
                    index.get_documents_with::<IdOnly>(&query),
                )
                .await?;
                let patches: Vec<AuthorNamePatch<'_>> = page
                    .results
                    .into_iter()
                    .map(|d| AuthorNamePatch {
                        id: d.id,
                        author_name: name,
                    })
                    .collect();
                let fetched = patches.len();
                if !patches.is_empty() {
                    let task = within(
                        self.write_timeout(),
                        "更新作者名",
                        index.add_or_update(&patches, Some("id")),
                    )
                    .await?;
                    self.remember(task);
                }
                if fetched < PAGE {
                    break;
                }
                offset += PAGE;
            }
        }
        Ok(())
    }

    // ---------- 查询 ----------

    pub(crate) async fn search_posts(
        &self,
        q: &str,
        category_slug: Option<&str>,
        excluded_authors: &[i64],
        sort: PostSort,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Hits> {
        let mut parts = Vec::new();
        if let Some(slug) = category_slug {
            parts.push(format!("categorySlug = {}", filter_string(slug)));
        }
        parts.extend(exclude_authors(excluded_authors));
        let filter = and(parts);
        let sort_by: &[&str] = match sort {
            PostSort::Relevance => &[],
            PostSort::Latest => &["bumpedAt:desc"],
            PostSort::Hot => &["likes:desc", "views:desc"],
        };
        let mut query = self.posts.search();
        query
            .with_query(q)
            .with_offset(offset)
            .with_limit(limit)
            .with_attributes_to_retrieve(Selectors::Some(&["id"]));
        if let Some(filter) = filter.as_deref() {
            query.with_filter(filter);
        }
        if !sort_by.is_empty() {
            query.with_sort(sort_by);
        }
        let results = within(self.timeout, "搜索帖子", query.execute::<IdOnly>()).await?;
        Ok(Hits {
            ids: parse_ids(&results.hits, |d| &d.id),
            total: results
                .estimated_total_hits
                .or(results.total_hits)
                .unwrap_or(results.hits.len()),
        })
    }

    /// 联想（SRCH-021）：标题高亮、正文裁剪 24 词。
    pub(crate) async fn suggest_posts(
        &self,
        q: &str,
        category_slug: Option<&str>,
        excluded_authors: &[i64],
        limit: usize,
    ) -> anyhow::Result<Vec<SuggestHit>> {
        let mut parts = Vec::new();
        if let Some(slug) = category_slug {
            parts.push(format!("categorySlug = {}", filter_string(slug)));
        }
        parts.extend(exclude_authors(excluded_authors));
        let filter = and(parts);
        let mut query = self.posts.search();
        query
            .with_query(q)
            .with_limit(limit)
            .with_attributes_to_retrieve(Selectors::Some(&["id"]))
            .with_attributes_to_highlight(Selectors::Some(&["title"]))
            .with_highlight_pre_tag(MARK_START)
            .with_highlight_post_tag(MARK_END)
            .with_attributes_to_crop(Selectors::Some(&[("text", None)]))
            .with_crop_length(24);
        if let Some(filter) = filter.as_deref() {
            query.with_filter(filter);
        }
        let results = within(self.timeout, "联想", query.execute::<IdOnly>()).await?;
        Ok(results
            .hits
            .into_iter()
            .filter_map(|hit| {
                let id = hit.result.id.parse().ok()?;
                let formatted = hit.formatted_result.unwrap_or_default();
                let field = |name: &str| {
                    formatted
                        .get(name)
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                };
                Some(SuggestHit {
                    id,
                    title_marked: field("title"),
                    excerpt: field("text"),
                })
            })
            .collect())
    }

    pub(crate) async fn search_comments(
        &self,
        q: &str,
        post_id: Option<i64>,
        excluded_authors: &[i64],
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Hits> {
        let mut parts = Vec::new();
        if let Some(post_id) = post_id {
            parts.push(format!("postId = {}", filter_string(&sid(post_id))));
        }
        parts.extend(exclude_authors(excluded_authors));
        let filter = and(parts);
        let mut query = self.comments.search();
        query
            .with_query(q)
            .with_offset(offset)
            .with_limit(limit)
            .with_attributes_to_retrieve(Selectors::Some(&["id"]));
        if let Some(filter) = filter.as_deref() {
            query.with_filter(filter);
        }
        let results = within(self.timeout, "搜索评论", query.execute::<IdOnly>()).await?;
        Ok(Hits {
            ids: parse_ids(&results.hits, |d| &d.id),
            total: results
                .estimated_total_hits
                .or(results.total_hits)
                .unwrap_or(results.hits.len()),
        })
    }

    /// 两个索引里的文档数（管理后台对账，ADMINCRUD-007）。
    pub(crate) async fn document_counts(&self) -> anyhow::Result<(usize, usize)> {
        let posts = within(self.write_timeout(), "读取索引统计", self.posts.get_stats()).await?;
        let comments = within(
            self.write_timeout(),
            "读取索引统计",
            self.comments.get_stats(),
        )
        .await?;
        Ok((posts.number_of_documents, comments.number_of_documents))
    }

    /// 测试用：删除两个索引。
    #[doc(hidden)]
    pub(crate) async fn drop_indexes(&self) -> anyhow::Result<()> {
        for index in [&self.posts, &self.comments] {
            let task = within(
                self.write_timeout(),
                "删除索引",
                self.client.delete_index(&index.uid),
            )
            .await?;
            within(
                Duration::from_secs(35),
                "删除索引",
                task.wait_for_completion(&self.client, None, Some(Duration::from_secs(30))),
            )
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_compare_ignores_order_of_sets_only() {
        let want = Settings::new()
            .with_searchable_attributes(["title", "text"])
            .with_filterable_attributes(["a", "b"])
            .with_sortable_attributes(["x", "y"])
            .with_ranking_rules(["words", "sort"]);
        let same = Settings::new()
            .with_searchable_attributes(["title", "text"])
            .with_filterable_attributes(["b", "a"])
            .with_sortable_attributes(["y", "x"])
            .with_ranking_rules(["words", "sort"]);
        assert!(settings_match(&want, &same));
        let reordered = same.clone().with_searchable_attributes(["text", "title"]);
        assert!(!settings_match(&want, &reordered));
        let changed = same.with_filterable_attributes(["a"]);
        assert!(!settings_match(&want, &changed));
    }

    #[test]
    fn author_exclusion_filter_quotes_ids() {
        assert_eq!(exclude_authors(&[]), None);
        assert_eq!(
            exclude_authors(&[1, 9_007_199_254_740_993]).as_deref(),
            Some(r#"NOT authorUserId IN ["1", "9007199254740993"]"#)
        );
    }
}
