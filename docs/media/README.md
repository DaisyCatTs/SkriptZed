# Screenshots for the README

Two images, both referenced from `README.md` as HTML comments until they exist.
Drop the files in this directory and replace the matching
`<!-- SCREENSHOT: … -->` line with the markdown given below.

Use **absolute `raw.githubusercontent.com` URLs**, not relative paths — the
registry mounts this repo as a submodule and any mirror of the README breaks
relative links.

---

## 1. `hero.png` — required

**Size:** 1400 × 420. Wide and short on purpose: a tall hero pushes `## Install`
off the first screenful, which defeats the point of putting it there.

**Setup**
- Theme: **One Dark** (Zed's default — it is what most viewers will be using
  themselves, so it reads as "this is what you will get").
- `"semantic_tokens": "combined"` **on**. This is the whole point — without it
  `teleport player to …` and `delete {homes::…}` render grey.
- Outline panel open on the left, with `/home` expanded so its entries show
  nested underneath.
- No window chrome, no title bar, no tab bar, no cursor, no selection, no
  squiggles.

**What to show:** `examples/sample-project/showcase.sk`, scrolled to the
`/home` command — roughly lines 40–56.

That block is the densest proof in the repo. In about sixteen lines it contains
the `command` structure keyword, `/home <text> [<text>]` argument specs, six
command entry keys, `trigger:`, three levels of nesting, `if`/`else if`/`else`,
`arg-1`, `{homes::%uuid of player%::%arg-2%}` (variable + list separator +
interpolation in one expression), a `{@prefix}` option reference, and `&a`/`&r`
colour codes inside a string.

**Markdown to paste:**

```markdown
![Skript highlighting in Zed](https://raw.githubusercontent.com/DaisyCatTs/SkriptZed/main/docs/media/hero.png)
```

---

## 2. `semantic-tokens.png` — required

The highest-value image in the repo. It exists to prevent the single most likely
first impression: *"I installed it and half my file is grey, it must be broken."*

**Size:** 1400 × 500 total, two panels side by side.

**Setup**
- Label each panel **inside the image**: `"semantic_tokens": "off"  (Zed's
  default)` on the left, `"combined"` on the right.
- Same twelve lines in both panels — `showcase.sk` lines 49–56, the trigger
  body. Every line there is statement prose, so the left panel is nearly
  monochrome and the right is fully lit. That contrast *is* the message.
- Same theme, same scroll position, same width. Nothing else may differ.

**Markdown to paste:**

```markdown
![The same file with semantic_tokens off and combined](https://raw.githubusercontent.com/DaisyCatTs/SkriptZed/main/docs/media/semantic-tokens.png)
```

---

## 3. `completion.gif` — optional

Do not hold the release for this one, but it is the best demo of the features
that are hardest to describe in a table.

**Constraints:** 8 seconds or less, 3 MB or less, 1000px wide, no loop delay.

**Sequence**
1. Type `on ` — the completion list appears containing only events.
2. Accept `on join:`, press Enter, type `send greet(` — signature help appears.
3. Move the cursor to an existing `add_points(player, 5)` call so the inlay
   hints `who:` and `amount:` are visible.

That shows completion, signature help and inlay hints in one take.

**Markdown to paste** (into `## What you get`, at the end):

```markdown
![Completion, signature help and inlay hints](https://raw.githubusercontent.com/DaisyCatTs/SkriptZed/main/docs/media/completion.gif)
```
