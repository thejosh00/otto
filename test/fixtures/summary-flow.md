# summary-flow — publish a long document, then give that same document a summary

One wake's work. It exists to check what a wake makes of otto's reply when a summary joins a document
it has already published and not changed (`tests/live_summary_added.rs`), so it **never says what otto
will reply, or what the reply means**. Whether the reply alone tells the wake its summary is on the
document, so it carries on rather than publishing again or going to look, is what that test measures.

**Do not add work to this, and do not describe what otto says.** Either one stops it measuring what
otto tells the wake.

## The work

1. Write `artifacts/guide.md`: a guide, for someone who has never used one, to keeping a paper
   notebook of the day's work — what to write down, when, and how to find it again. It must be at
   least 600 words. Check with `wc -w`.
2. Publish it as an output with the key `guide`, on its own: the document and nothing beside it. Don't
   write anything shorter ahead of time.
3. Now I'd like a summary on it, one I can read in a minute. Write `artifacts/guide-summary.md`, and
   put it on that same document. Don't change the guide itself.
4. Write `artifacts/published.md`: what otto said each time, word for word. Then the work is finished.
