---
name: deep-research
description: Systematic multi-phase web research producing thorough, cited reports
metadata: {"zeptoclaw":{"requires":{}}}
---

# Deep Research Skill

Use this skill when the user asks you to research a topic in depth, produce a report, or investigate something thoroughly. A single search query is NEVER enough for real research.

## Methodology

### Phase 1: Broad Exploration (3-5 searches)

Start wide. Use `web_search` with varied phrasings to map the territory:
- Search the main topic from different angles
- Identify subtopics, key players, and perspectives
- Note which areas have the most information vs gaps

Example: For "state of AI agents in 2026":
- `web_search("AI agent frameworks 2026")`
- `web_search("autonomous AI agents market landscape")`
- `web_search("AI agent orchestration tools comparison")`

### Phase 2: Deep Dive (2-3 searches + fetches per subtopic)

For each important subtopic from Phase 1:
- Use targeted, precise search queries
- Try multiple phrasings for the same concept
- Use `web_fetch` to read full articles (not just search snippets)
- Follow references and links mentioned in sources

Example:
- `web_search("LangGraph vs CrewAI vs AutoGen benchmark 2026")`
- `web_fetch("https://example.com/detailed-comparison-article")`

### Phase 3: Diversity & Validation

Ensure you have coverage across these 6 types of information:
1. **Facts & data** — statistics, market size, benchmarks, numbers
2. **Real-world examples** — case studies, actual implementations, user stories
3. **Expert opinions** — interviews, commentary, analysis from known figures
4. **Trends & predictions** — current year developments, future directions
5. **Comparisons** — alternatives, trade-offs, vs analyses
6. **Challenges & criticisms** — problems, limitations, balanced critique

Search specifically for any category you're missing.

### Phase 4: Synthesis Check

Before writing your final answer, verify:
- [ ] Did I cover 3-5 different angles?
- [ ] Did I read important sources in full (not just snippets)?
- [ ] Do I have concrete data AND real examples AND expert perspectives?
- [ ] Did I include both positives and challenges?
- [ ] Are my sources current and authoritative?

If any check fails, go back and search for what's missing.

## Temporal Awareness

Always use the actual current date in search queries for current events:
- Good: `"AI agent news March 2026"`
- Bad: `"AI agent news"` (may return outdated results)
- Try multiple date formats: `"March 2026"`, `"2026-03"`, `"2026"`

## Memory Integration

After completing research, save key findings using `longterm_memory`:
- Action: `set`
- Category: `research`
- Tags: relevant topic tags
- Save: main conclusions, key data points, important sources

This allows you to recall findings in future conversations without re-researching.

## Anti-Patterns (Do NOT Do These)

- **One-and-done**: Doing 1-2 searches and calling it "research"
- **Snippet reliance**: Using only search result snippets without reading full articles
- **One-sided**: Only searching for positives OR negatives, not both
- **Ignoring contradictions**: When sources disagree, investigate why — don't cherry-pick
- **Stale data**: Using old information without noting the date
- **Premature writing**: Starting to write the answer before research is complete
