use anyhow::Result;
use bsky_sdk::api::app::bsky::embed::record::ViewRecordRefs;
use bsky_sdk::api::app::bsky::feed::defs::{FeedViewPostData, PostViewData, PostViewEmbedRefs};
use bsky_sdk::api::app::bsky::richtext::facet::MainFeaturesItem;
use bsky_sdk::api::types::{Object, TryFromUnknown, Union};
use megalodon::entities::Status;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use unicode_segmentation::UnicodeSegmentation;

// Represents new status updates that should be posted to Bluesky (bsky_posts)
// and Mastodon (toots).
#[derive(Debug, Clone)]
pub struct StatusUpdates {
    pub bsky_posts: Vec<NewStatus>,
    pub toots: Vec<NewStatus>,
}

impl StatusUpdates {
    /// Reverses the order of statuses in place.
    pub fn reverse_order(&mut self) {
        self.bsky_posts.reverse();
        self.toots.reverse();
    }
}

// A new status for posting. Optionally has links to media (images) that should
// be attached.
#[derive(Debug, Clone)]
pub struct NewStatus {
    pub text: String,
    pub attachments: Vec<NewMedia>,
    // A list of further statuses that are new replies to this new status. Used
    // to sync threads.
    pub replies: Vec<NewStatus>,
    // This new status could be part of a thread, post it in reply to an
    // existing already synced status.
    pub in_reply_to_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewMedia {
    pub attachment_url: String,
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub sync_reblogs: bool,
    pub sync_reposts: bool,
    pub sync_hashtag_bluesky: Option<String>,
    pub sync_hashtag_mastodon: Option<String>,
}

/// This is the main synchronization function that can be tested without
/// external API calls.
///
/// The ordering of the statuses in both list parameters is expected to be from
/// newest to oldest. That is also the ordering returned by the Bluesky and
/// Mastodon APIs for their timelines, they start with newest posts first.
///
/// The returned data structure contains new posts that are not synchronized yet
/// and should be posted on both Bluesky and Mastodon. They are ordered in
/// reverse so that older statuses are posted first if there are multiple
/// statuses to synchronize.
pub fn determine_posts(
    mastodon_statuses: &[Status],
    bsky_statuses: &[Object<FeedViewPostData>],
    options: &SyncOptions,
) -> StatusUpdates {
    let mut updates = StatusUpdates {
        bsky_posts: Vec::new(),
        toots: Vec::new(),
    };
    'bsky: for post in bsky_statuses {
        // Skip replies, they are handled in determine_thread_replies().
        if let Some(_reply) = &post.reply {
            continue;
        }

        if !options.sync_reposts {
            if let Some(_reskeet) = &post.post.viewer {
                if let Some(_repost) = &_reskeet.repost {
                    // Skip reskeets when sync_reposts is disabled
                    continue;
                }
            }
        }

        for toot in mastodon_statuses {
            // Skip replies because we don't want to sync them here.
            if let Some(_id) = &toot.in_reply_to_id {
                continue;
            }
            // If the post already exists we can stop here and know that we are
            // synced.
            if toot_and_post_are_equal(toot, post) {
                break 'bsky;
            }
        }

        // The post is not on Mastodon yet, check if we should post it.
        // Fetch the post text into a String object
        let decoded_post = bsky_post_unshorten_decode(post);

        // Check if hashtag filtering is enabled and if the post matches.
        if let Some(sync_hashtag) = &options.sync_hashtag_bluesky {
            if !sync_hashtag.is_empty() && !decoded_post.contains(sync_hashtag) {
                // Skip if a sync hashtag is set and the string doesn't match.
                continue;
            }
        }

        updates.toots.push(NewStatus {
            text: decoded_post,
            attachments: bsky_get_attachments(post),
            replies: Vec::new(),
            in_reply_to_id: None,
        });
    }

    'toots: for toot in mastodon_statuses {
        // Skip replies, they are handled in determine_thread_replies().
        if let Some(_id) = &toot.in_reply_to_id {
            continue;
        }

        if toot.reblog.is_some() && !options.sync_reblogs {
            // Skip reblogs when sync_reblogs is disabled
            continue;
        }
        let fulltext = mastodon_toot_get_text(toot);
        // If this is a reblog/boost then take the URL to the original toot.
        let post = match &toot.reblog {
            None => bsky_post_shorten(&fulltext, &toot.url),
            Some(reblog) => bsky_post_shorten(&fulltext, &reblog.url),
        };
        // Skip direct toots to other Mastodon users, even if they are public.
        if post.starts_with('@') {
            continue;
        }

        for bsky_post in bsky_statuses {
            // If the toot already exists we can stop here and know that we are
            // synced.
            if toot_and_post_are_equal(toot, bsky_post) {
                break 'toots;
            }
        }

        // The toot is not on Bluesky yet, check if we should post it.
        // Check if hashtag filtering is enabled and if the post matches.
        if let Some(sync_hashtag) = &options.sync_hashtag_mastodon {
            if !sync_hashtag.is_empty() && !fulltext.contains(sync_hashtag) {
                // Skip if a sync hashtag is set and the string doesn't match.
                continue;
            }
        }

        updates.bsky_posts.push(NewStatus {
            text: post,
            attachments: toot_get_attachments(toot),
            replies: Vec::new(),
            in_reply_to_id: None,
        });
    }

    //determine_thread_replies(mastodon_statuses, bsky_statuses, options, &mut updates);

    // Older posts should come first to preserve the ordering of posts to
    // synchronize.
    updates.reverse_order();
    updates
}

/*fn bsky_post_is_reply(post: &Object<FeedViewPostData>) -> bool {
    if let Some(_reskeet) = &post.post.viewer {
        if let Some(_repost) = _reskeet.repost {
            // Skip retweets when sync_retweets is disabled
            continue;
        }
    }
}*/

// Returns true if a Mastodon toot and a Bluesky post are considered equal.
pub fn toot_and_post_are_equal(toot: &Status, bsky_post: &Object<FeedViewPostData>) -> bool {
    // Make sure the structure is the same: both must be replies or both must
    // not be replies.
    if (toot.in_reply_to_id.is_some() && bsky_post.reply.is_none())
        || (toot.in_reply_to_id.is_none() && bsky_post.reply.is_some())
    {
        return false;
    }

    // Strip markup from Mastodon toot and unify message for comparison.
    let toot_text = unify_post_content(mastodon_toot_get_text(toot));
    // Populate URLs in the post text.
    let bsky_text = unify_post_content(bsky_post_unshorten_decode(bsky_post));

    if toot_text == bsky_text {
        return true;
    }
    // Mastodon allows up to 500 characters, so we might need to shorten the
    // toot. If this is a reblog/boost then take the URL to the original toot.
    let shortened_toot = unify_post_content(match &toot.reblog {
        None => bsky_post_shorten(&toot_text, &toot.url),
        Some(reblog) => bsky_post_shorten(&toot_text, &reblog.url),
    });

    if shortened_toot == bsky_text {
        return true;
    }

    false
}

// Unifies bluesky text or toot text to a common format.
fn unify_post_content(content: String) -> String {
    let mut result = content.to_lowercase();
    // Remove http:// and https:// for comparing because Bluesky sometimes adds
    // those randomly.
    result = result.replace("http://", "");
    result = result.replace("https://", "");

    // Escape direct user mentions with \@.
    result = result.replace(" \\@", " @");
    result.replace(" @\\", " @")
}

// Extend URLs and HTML entity decode &amp;.
// Directly include quoted posts in the text.
pub fn bsky_post_unshorten_decode(bsky_post: &Object<FeedViewPostData>) -> String {
    let record = bsky_sdk::api::app::bsky::feed::post::RecordData::try_from_unknown(
        bsky_post.post.record.clone(),
    )
    .expect("Failed to parse Bluesky post record");
    let mut text = bsky_record_get_text(record);

    // Add prefix for reposts.
    if let Some(viewer) = &bsky_post.post.viewer {
        if let Some(_repost) = &viewer.repost {
            text = format!("♻️ {}: {}", bsky_post.post.author.handle.as_str(), text);
        }
    }

    if let Some(embed) = &bsky_post.post.embed {
        if let Union::Refs(PostViewEmbedRefs::AppBskyEmbedRecordView(embed_record)) = &embed {
            if let Union::Refs(ViewRecordRefs::ViewRecord(quote)) = &embed_record.record {
                let quote_record =
                    bsky_sdk::api::app::bsky::feed::post::RecordData::try_from_unknown(
                        quote.value.clone(),
                    )
                    .expect("Failed to parse Bluesky quote post record");
                let quote_text = bsky_record_get_text(quote_record);
                text = format!(
                    "{text}\n\n💬 {}: {quote_text}",
                    quote.author.handle.as_str()
                );
            }
        }
    }
    toot_shorten(&text, &bsky_post.post)
}

// Get the full text of a bluesky post.
fn bsky_record_get_text(bsky_record: bsky_sdk::api::app::bsky::feed::post::RecordData) -> String {
    let mut text = bsky_record.text.clone();
    // Convert links in facets to URIs in the text.
    if let Some(facets) = &bsky_record.facets {
        for facet in facets {
            if let Union::Refs(MainFeaturesItem::Link(link)) = &facet.features[0] {
                let mut bytes = bsky_record.text.as_bytes().to_vec();
                bytes.splice(
                    facet.index.byte_start..facet.index.byte_end,
                    link.uri.as_bytes().iter().cloned(),
                );
                text = String::from_utf8(bytes)
                    .expect("Invalid UTF-8 in Bluesky post after replacing link");
            }
        }
    }
    text
}

pub fn bsky_post_shorten(text: &str, toot_url: &Option<String>) -> String {
    let mut char_count = text.graphemes(true).count();
    let re = Regex::new(r"[^\s]+$").unwrap();
    let mut shortened = text.trim().to_string();
    let mut with_link = shortened.clone();

    // Bluesky has a limit of 300 characters.
    while char_count > 300 {
        // Remove the last word.
        shortened = re.replace_all(&shortened, "").trim().to_string();
        if let Some(ref toot_url) = *toot_url {
            // Add a link to the toot that has the full text.
            with_link = shortened.clone() + "… " + toot_url;
        } else {
            with_link = shortened.clone();
        }
        let new_count = with_link.graphemes(true).count();
        char_count = new_count;
    }
    with_link
}

// Mastodon has a 500 character post limit. With embedded quote posts and long
// links the content could get too long, shorten it to 500 characters.
fn toot_shorten(text: &str, bsky_post: &Object<PostViewData>) -> String {
    let mut char_count = text.graphemes(true).count();
    // Hard-coding a limit of 500 here for now, could be configurable.
    if char_count <= 500 {
        return text.to_string();
    }
    let last_word_regex = Regex::new(r"[^\s]+$").unwrap();
    let mut shortened = text.trim().to_string();
    let mut with_link = shortened.clone();
    let username = bsky_post.author.handle.as_str();
    // Get everything after the last slash, example:
    // at://did:plc:i7uartkbj7ktzo4tj4rq6oyi/app.bsky.feed.post/3lb3f2ko4rc23
    let post_id_regex = Regex::new(r"[^/]+$").unwrap();
    let post_id = post_id_regex
        .find(&bsky_post.uri)
        .map(|mat| mat.as_str())
        .unwrap();
    let link = format!("https://bsky.app/profile/{username}/post/{post_id}");

    while char_count > 500 {
        // Remove the last word.
        shortened = last_word_regex
            .replace_all(&shortened, "")
            .trim()
            .to_string();
        // Add a link to the full length post on Bluesky.
        with_link = format!("{shortened}… {link}");
        char_count = with_link.graphemes(true).count();
    }
    with_link
}

// Prefix boost toots with the author and strip HTML tags.
pub fn mastodon_toot_get_text(toot: &Status) -> String {
    let mut replaced = match toot.reblog {
        None => toot.content.clone(),
        Some(ref reblog) => format!("♻️ {}: {}", reblog.account.username, reblog.content),
    };
    replaced = replaced.replace("<br />", "\n");
    replaced = replaced.replace("<br>", "\n");
    replaced = replaced.replace("</p><p>", "\n\n");
    replaced = replaced.replace("<p>", "");
    replaced = replaced.replace("</p>", "");

    replaced = voca_rs::strip::strip_tags(&replaced);

    // Escape direct user mentions with @\.
    replaced = replaced.replace(" @", " @\\").replace(" @\\\\", " @\\");

    html_escape::decode_html_entities(&replaced).to_string()
}

// Ensure that sync posts have not been made before to prevent syncing loops.
// Use a cache file to temporarily store posts and compare them on the next
// invocation.
pub fn filter_posted_before(
    posts: StatusUpdates,
    post_cache: &HashSet<String>,
) -> Result<StatusUpdates> {
    // If there are no status updates then we don't need to check anything.
    if posts.toots.is_empty() && posts.bsky_posts.is_empty() {
        return Ok(posts);
    }

    let mut filtered_posts = StatusUpdates {
        bsky_posts: Vec::new(),
        toots: Vec::new(),
    };
    for post in posts.bsky_posts {
        if post_cache.contains(&post.text) {
            eprintln!("Error: preventing double posting to Bluesky: {}", post.text);
        } else {
            filtered_posts.bsky_posts.push(post.clone());
        }
    }
    for toot in posts.toots {
        if post_cache.contains(&toot.text) {
            eprintln!(
                "Error: preventing double posting to Mastodon: {}",
                toot.text
            );
        } else {
            filtered_posts.toots.push(toot.clone());
        }
    }

    Ok(filtered_posts)
}

// Read the JSON encoded cache file from disk or provide an empty default cache.
pub fn read_post_cache(cache_file: &str) -> HashSet<String> {
    match fs::read_to_string(cache_file) {
        Ok(json) => {
            match serde_json::from_str::<HashSet<String>>(&json) {
                Ok(cache) => {
                    // If the cache has more than 150 items already then empty it to not
                    // accumulate too many items and allow posting the same text at a
                    // later date.
                    if cache.len() > 150 {
                        HashSet::new()
                    } else {
                        cache
                    }
                }
                Err(_) => HashSet::new(),
            }
        }
        Err(_) => HashSet::new(),
    }
}

// Returns a list of direct links to attachments for download.
pub fn bsky_get_attachments(bsky_post: &Object<FeedViewPostData>) -> Vec<NewMedia> {
    let mut links = Vec::new();

    if let Some(embed) = &bsky_post.post.embed {
        if let Union::Refs(refs) = embed {
            match &refs {
                PostViewEmbedRefs::AppBskyEmbedImagesView(ref image_box) => {
                    let images = &image_box.images;
                    for image in images {
                        links.push(NewMedia {
                            attachment_url: image.fullsize.clone(),
                            alt_text: Some(image.alt.clone()),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    links
}

// Returns a list of direct links to attachments for download.
pub fn toot_get_attachments(toot: &Status) -> Vec<NewMedia> {
    let mut links = Vec::new();
    let mut attachments = &toot.media_attachments;
    // If there are no attachments check if this is a boost and if there might
    // be some attachments there.
    if attachments.is_empty() {
        if let Some(boost) = &toot.reblog {
            attachments = &boost.media_attachments;
        }
    }
    for attachment in attachments {
        links.push(NewMedia {
            attachment_url: attachment.url.clone(),
            // Bluesky only allows a max length of 1,000 characters for alt
            // text, so we need to cut it off here.
            alt_text: truncate_option_string(attachment.description.clone(), 1_000),
        });
    }
    links
}

/// Truncates a given string to a maximum number of characters.
///
/// I could not find a Rust core function that does this? We don't care about
/// graphemes, please just cut off characters at a certain length. Copied from
/// https://stackoverflow.com/a/38461750/2000435
///
/// No, I will not install the substring crate just to get a substring, are you
/// kidding me????
fn truncate_option_string(stringy: Option<String>, max_chars: usize) -> Option<String> {
    match stringy {
        Some(string) => match string.char_indices().nth(max_chars) {
            None => Some(string),
            Some((idx, _)) => Some(string[..idx].to_string()),
        },
        None => None,
    }
}

#[cfg(test)]
pub mod tests {
    use bsky_sdk::api::app::bsky::feed::defs::FeedViewPostData;
    use bsky_sdk::api::types::Object;
    use std::fs;

    use crate::{determine_posts, sync::toot_shorten, SyncOptions};

    // Test that embedded quote posts are included correctly.
    #[test]
    fn bsky_quote_post() {
        let post = read_bsky_post_from_json("tests/bsky_quote_post.json");
        let posts = determine_posts(&Vec::new(), &vec![post], &SyncOptions::default());
        assert_eq!(
            posts.toots[0].text,
            "Working on this and testing quote posts

💬 klau.si: Initial release of #Mastodon #Bluesky Sync 🚀  !

Synchronization of posts works, but I'm still testing things.

https://github.com/klausi/mastodon-bluesky-sync/releases/tag/v0.2.0"
        );
    }

    // Test that a correct Bluesky link is appended when posting to Mastodon.
    #[test]
    fn toot_shorten_link() {
        let text = "a ".repeat(251);
        let post = read_bsky_post_from_json("tests/bsky_quote_post.json");
        let expected = format!(
            "{}a… https://bsky.app/profile/klau.si/post/3lb3f2ko4rc23",
            "a ".repeat(223)
        );
        assert_eq!(expected, toot_shorten(&text, &post.post));
    }

    // Read static bluesky post from test file.
    fn read_bsky_post_from_json(file_name: &str) -> Object<FeedViewPostData> {
        let json = fs::read_to_string(file_name).unwrap();
        serde_json::from_str(&json).unwrap()
    }
}
