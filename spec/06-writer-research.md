# Writer research (WP11)

What the research literature says about the screen and the environment that
help a person write, and what this means for Carrel. This is the input for
WP12 (UI review). It does not change any rule in `00-rules.md`.

**Evidence labels** used in every finding and rule:

| Label | Meaning |
|---|---|
| **Strong** | Meta-analysis, systematic review, or an effect replicated by several independent groups. |
| **Moderate** | Several studies point the same way, but samples are small, settings differ, or there is no meta-analysis. |
| **Weak** | One study, a small sample, an indirect analogy from another domain, or mixed results. |
| **Opinion** | Practice, craft knowledge, or design taste. No real evidence either way. |

References are numbered `[n]` and listed at the end with links. Where a
number below comes only from an abstract or a secondary summary (not the full
paper), this is said in the text.

---

## 1. Summary (one page)

**The main finding is negative and important: almost nothing in a
"distraction-free writing device" has been tested directly.** We found no
peer-reviewed study of the Freewrite, of iA Writer-style focus modes, of
typewriter scrolling, or of dimming paragraphs. The design must therefore lean
on nearby, better-tested research: reading and legibility, interruptions and
task resumption, cognitive models of writing, goal monitoring, and motivation
theory. Each of these is fairly solid on its own.

What the evidence supports well:

1. **Interruptions are expensive, even very short ones.** Interruptions of
   about 3 s double sequence errors in a demanding task [40]; interrupted work
   is done faster but with more stress [41]; phone notifications raise
   self-reported inattention [45]. Most interruptions at a computer are
   self-started [50], and blocking the sources helps focus [49]. *Strong for
   the cost, Moderate for blocking.* Carrel's core idea (one app, network off,
   no notifications) is the best-supported part of the design.
2. **Dark text on a light background reads better than the reverse** for
   people with normal vision, in bright and in dim rooms [22–26]. *Strong.*
   "Dark mode is easier on the eyes" is not supported. A dark theme is a
   comfort choice for dark rooms, not a health feature, and on an LCD laptop
   it saves almost no battery.
3. **Text size matters more than font, line height or line length** within
   normal ranges [17, 18]. Text must stay well above the critical print size
   (about 0.2° x-height). *Strong.* Serif vs sans makes no reliable
   difference [20]; fixed-width fonts are slightly slower to read for normal
   vision [19]. *Moderate.*
4. **Writing is a juggle of planning, putting words down and reviewing
   inside a small working memory** [1, 2, 4]. Tools help when they take load
   away and hurt when they add a parallel task (correcting, judging, watching
   numbers). *Strong as theory, Moderate for specific tool effects.*
5. **Writers re-read their own text all the time while drafting**; strong
   typists look at the screen and re-read more [11]. Hiding the text does not
   clearly hurt quality but changes how the writer coordinates the
   work [9]. *Weak.* There is no case for hiding or strongly dimming text by
   default.
6. **Watching progress toward a goal helps people reach it**, more when the
   progress is recorded [52]. Specific goals help [56]. *Strong.* But a
   progress bar does not on average keep people going [55], and **streaks
   backfire when they break** [70]. *Moderate.*
7. **Expected rewards, points and badges can undermine interest in a task
   people already like** [63, 66, 67]; informational feedback does
   not [63, 65]. *Strong for tangible rewards, Moderate for gamification.*
8. **Predictive text makes writing shorter and more predictable** [12].
   *Weak–Moderate.* This supports rule 9 (no AI) and argues against any
   autocomplete or autocorrect during drafting.
9. **People tend to resume interrupted tasks** (Ovsiankina effect), but the
   popular "unfinished tasks stay in memory" claim (Zeigarnik effect) does not
   hold up in a recent meta-analysis [61]. *Strong.* Good resume cues (the
   exact place and the text around it) shorten the restart [43, 44].
   *Moderate.*

Popular claims the evidence does **not** support: dark mode is easier on the
eyes; blue light at night "ruins" sleep (the measured effects are real but
modest, and night-shift colour filters showed no sleep benefit [28, 29]);
blue-blocking glasses reduce eye strain [30]; monospace fonts reduce
perfectionism (no study found); hard-to-read fonts make you think harder [83];
the mere presence of a phone drains cognition [47, 48]; unfinished work is
remembered better (Zeigarnik) [61]; moderate background noise boosts
creativity (single study, not replicated) [76].

Where the current Carrel design **disagrees** with the evidence (details in
section 5):

- Body text is specified in **pixels**. On high-DPI laptop panels 19 px can
  fall near or below the critical print size. Size must follow physical
  size / visual angle.
- **Focus mode at 35 % opacity** gives 2.1:1 (Paper) and 2.6:1 (Night)
  contrast. That is below any legibility floor, and writers re-read the
  previous paragraph constantly.
- **Dim text at 55 % in Paper** is 3.5:1, below the 4.5:1 text minimum, if it
  is used for any text the writer must read (bottom line, file details).
- **Text on the Paper accent** (selected row) cannot reach 4.5:1 with either
  ink (3.6:1) or paper (3.9:1).

---

## 2. Findings per topic

### 2.1 Typography and screen ergonomics

**F1 · Text size is the main typographic lever. — Strong.**
Reading speed is flat across a wide "fluent range" of sizes and drops sharply
below the critical print size, about 0.2° of visual angle for the x-height in
normal vision; for comfortable, sustained reading the range of roughly
0.2°–2° applies, and many book and screen sizes sit just above the lower
edge [18]. Rello et al. (CHI 2016, 104 readers, eye tracking) found that on
screen, readability and comprehension kept improving up to 18 pt (24 px) and
that 10–12 pt hurt comprehension [17] (numbers from the authors' slides and
summaries). *Consequence:* for a laptop at about 55 cm, aim for an x-height of
0.25°–0.3° (2.4–2.9 mm) by default and never below 0.2° (1.9 mm). 19 px is
fine on a ~96 ppi panel, at the floor on a ~110 ppi panel, and too small on a
160+ ppi panel without scaling (see section 5).

**F2 · Line length in the 55–80 character range is safe; the exact number
matters little. — Moderate.**
Dyson & Kipping found that on screen, longer lines (up to ~100 characters)
were read *faster* when scrolling, but readers preferred and rated as
easier the medium length (~55 characters); comprehension did not differ much
[15, 16]. WCAG 2.2 success criterion 1.4.8 (AAA) asks for lines of no more
than 80 characters [37]. Typographers recommend 45–75 [84] (*Opinion*, but
consistent with the data). For *composing* (not reading), no study tested
line length. Carrel's 60–75 is inside every recommended range.

**F3 · Line height has small effects within normal values. — Moderate.**
Rello et al. found little effect of line spacing between 0.8 and 1.8, with
only the tightest (0.8) hurting comprehension [17]. Wider spacing helps in
peripheral vision, presumably by reducing crowding [21]. WCAG 1.4.8 asks for
at least 1.5 within paragraphs and 1.4.12 requires that layouts survive 1.5
[37]. Carrel's 1.6 meets this; going much higher brings no proven benefit and
costs lines on a small laptop screen.

**F4 · Serif vs sans: no reliable difference. — Moderate.**
Controlled studies find serifs neither help nor hurt legibility at normal
sizes [20]. Font preference is personal and is a legitimate comfort setting.

**F5 · Fixed-width (mono) fonts are slightly slower for normal vision. —
Moderate.**
Mansfield, Legge & Bane: maximum reading speed was about 5 % faster with
Times than Courier for 50 normally sighted readers; for low vision and for
very small print, Courier was better [19]. *Unsupported claim:* "monospace
reduces perfectionism / makes drafting feel provisional". We found no study.
It is a craft belief (*Opinion*). Offering mono as an option is fine; it
should not be the default.

**F6 · Hard-to-read fonts do not make people think or write better. —
Moderate (failed replications).**
The "disfluency" benefit [Diemand-Yauman et al. 2011] failed to replicate in
large studies [83]. Do not use deliberately rough type as a feature. (The
writer's practice of changing font *to proofread* is untested — *Opinion*.)

**F7 · Positive polarity (dark text on light background) beats negative
polarity for normal vision. — Strong.**
Replicated across labs and decades: better proofreading and legibility with
positive polarity, for young and older adults [22, 25], in bright and dim
ambient light [25, 26], largest for small characters [24]. The leading
explanation is pupil size: a bright background makes the pupil smaller, which
sharpens the retinal image [23]. In the dark-room glance study, light-on-dark
text in a dark room was the *worst* condition [26]. *Unsupported claim:* "dark
mode is easier on the eyes". *Nuance:* some people with cataract or other
light-scatter conditions prefer and do better with negative polarity
(*Moderate*, clinical literature summarised in [24]); this is the real case
for a Night theme, plus personal comfort in a dark room (*Opinion*).

**F8 · Dark themes save power only on OLED. — Strong (physics) / Moderate
(measurement).**
On OLED phones, dark mode saves 39–47 % of display power at full brightness
but only 3–9 % at 30–50 % brightness [27]. Laptop LCDs have a backlight that
burns the same power whatever the pixels show; on LCD, the brightness level
is what drives display power. (Press summary of [27]; the LCD point follows
from how the backlight works.)

**F9 · Very bright screens increase visual fatigue. — Weak–Moderate.**
High screen luminance reduced blink rate and increased fatigue in a reading
study [32]. Digital eye strain is common and linked to reduced blinking, long
sessions and poor viewing conditions [34]. Practical consequence: brightness
should match the room, and the brightness keys must work (they are on the
hardware checklist, `04-hardware.md`).

**F10 · Colour temperature at night: real but modest effects; colour filters
did not help sleep. — Moderate.**
Reading on a bright iPad for 4 h before bed (vs a printed book) suppressed
melatonin, delayed the circadian clock and REM sleep, and made people slower
to wake up; sleep onset was about 10 minutes later (12 participants;
minute figure from secondary summaries) [28]. A randomised field study of
iPhone "Night Shift" found no sleep difference between Night Shift on, off,
or no phone at all, in the full sample [29]. A Cochrane review found that
blue-blocking spectacles probably do not reduce eye strain, and sleep effects
are uncertain [30]. *Unsupported claim:* "blue light at night ruins sleep" in
the sense of large effects from a normal-brightness writing screen; the
magnitude claims are overstated, and engagement and brightness seem to matter
as much as spectrum [29]. Carrel has no engaging feeds, which removes the
part of the problem that is best supported.

**F11 · E-ink vs LCD: small and inconsistent differences. — Weak.**
One study found more visual fatigue (fewer blinks, higher fatigue score) on
an LCD tablet than on e-ink or paper [31]; another found no difference in
reading effort (EEG and eye tracking) between paper, e-ink and LCD, even
though readers *said* they preferred paper [33]. Carrel runs on laptop LCDs;
this is not a reason to change course, but it supports keeping the LCD dim
and steady (no flashing, no animation).

**F12 · Reading on paper beats reading on screen for comprehension of
informational text, especially under time pressure. — Strong.**
Meta-analysis of 54 studies, about 171 000 readers: g = −0.21 for screen,
larger under time limits, absent for narrative text [36]. For writers this
matters at the *revision* stage (reading one's own draft critically), not
drafting. PDF export and printing are a sensible part of the revision loop.
(*Inference, Weak* for writing specifically.)

**F13 · Glare and reflections. — Opinion / standards.**
Display standards (ISO 9241-303 [38]) treat reflections as a loss of
contrast and require minimum contrast under the expected room light. No
writing-specific evidence. A warm off-white rather than pure white lowers
peak luminance slightly; the choice of warm tint itself is taste (*Opinion*).

### 2.2 Attention and distraction

**F14 · Task switching and interruptions have a measurable cost. — Strong.**
Switching between tasks costs time and accuracy even when the switch is
expected [39]. Interruptions of ~2.8 s doubled, and ~4.4 s tripled, sequence
errors in a procedural task (300 participants) [40]. Interrupted office work
was finished faster, at the same quality, but with more stress, frustration
and effort [41]. The memory-for-goals model explains why: the suspended goal
decays and must be rebuilt from cues [42]. In field logs, people often took
many minutes to get back to a suspended task after an alert, and often
visited other applications on the way [85].

**F15 · Notifications harm attention; most interruptions are
self-started. — Moderate.**
A week with phone alerts on vs off raised self-reported inattention and
hyperactivity in 221 students [45]. Merely receiving an (unanswered)
notification hurt performance on an attention task [46]. Delivering
notifications in batches a few times a day, instead of at once, improved
well-being and attention in a field experiment [79]. In field studies of
office workers, about half of interruptions were self-interruptions [50], and
software that blocked distracting sites raised self-rated focus and
productivity, most for people low in self-control (32 workers) [49].
Carrel's design (no browser, network off, no notifications) removes both the
external and the self-started paths.

**F16 · The "brain drain" from a phone's mere presence did not
replicate. — Moderate.**
The original finding [47] failed a pre-registered direct replication with
~380 participants [48]. Do not market Carrel with this claim. The supported
claim is narrower: removing the *ability* to switch (F15) helps.

**F17 · Motion and sudden onsets capture attention automatically. —
Strong (vision science).**
Motion onset captures attention even when it is irrelevant [51]. A blinking
caret, animated counters, sliding toasts and pulsing indicators are all
motion onsets. This supports Carrel's "no animation, no caret blink" rule.
WCAG 2.3.3 also asks that motion from interaction can be turned off [37].

**F18 · Focus mode (dimming other paragraphs): no evidence. — Opinion.**
We found no study of paragraph dimming. Two related findings argue for
caution: writers re-read earlier text frequently while composing, and
fluent typists re-read more [11]; and legibility drops steeply once contrast
falls toward 3:1 and below (WCAG uses 4.5:1 for body text and 3:1 for large
text [37]). A strong dim works against the re-reading that drafting needs.
A light dim (context still legible) is a reasonable comfort option.

**F19 · Typewriter scrolling: no evidence. — Opinion.**
No study found. The plausible benefits are a steady gaze height and less
neck flexion on a low laptop screen (the line being written does not sink to
the bottom). The plausible cost is losing the "page" as a spatial memory.
Keep it (it fits the device), but do not claim it is evidence-based.

**F20 · Writing without seeing the text: quality little changed, process
changed. — Weak.**
Suppressing visual feedback had little or no effect on text quality but
made writers run low-level and high-level processes step by step instead of
in parallel [9]. Blau's classroom "invisible writing" study is descriptive
[10]. Monitor-gazing typists (who see and re-read their text) wrote faster
and more fluently than keyboard-gazers, with similar final quality [11].
Conclusion: hiding text is a niche exercise (some freewriting tools offer
it), not a default. Carrel does not hide text; this is right.

**F21 · Visible word counts and goals. — Strong (monitoring), Moderate
(display form), no direct writing study.**
Interventions that make people monitor progress raise goal attainment
(138 studies, ~20 000 people, d ≈ 0.40), more when progress is *recorded*
or reported [52] (numbers partly from secondary summaries). Specific,
challenging goals beat "do your best" [56]. Motivation rises as a goal gets
closer (goal gradient) [54]. Attention to the *smaller* area helps: early on,
show what is done; near the end, show what remains [53]. But a constant-speed
progress bar did not on average reduce drop-out across 32 experiments, and
slow-then-fast bars increased it [55]. There is no study of a live word count
during composition. Writers' reports that a running count causes anxiety or
"writing to the number" are *Opinion*; they are plausible given the
working-memory view (F24) and Carrel already lets the count be hidden (F1 key,
Hairline).

**F22 · Spellcheck and autocorrect during drafting. — Weak / Opinion.**
No controlled study isolates "red underlines while drafting" for skilled
adults. Theory predicts a cost: stopping to fix surface errors breaks a
translation burst and loads working memory [4, 8]. Readers do judge authors
harshly for spelling errors in typed text [13], so spellcheck belongs to the
revision pass. Predictive text (suggesting the next word) changed *content*:
shorter, more predictable captions, with 109 writers [12]. Autocorrect is a
close cousin (it silently changes words) and has no supporting evidence for
drafting quality.

### 2.3 Writing process research

**F23 · Writing is recursive planning, translating and reviewing under
a monitor. — Strong (as a framework).**
Hayes & Flower's model [1] and its revisions [2] describe writing as
planning, translating (putting ideas into words), and reviewing, with
constant switching between them, not as fixed stages. Bereiter & Scardamalia
contrast "knowledge telling" with "knowledge transforming" [7].

**F24 · Working memory is the bottleneck. — Strong.**
Kellogg's account: the three processes compete for a limited working memory;
expert writers manage this load by habit and strategy [4]. Words come in
"bursts" between pauses; burst length grows with language skill and falls
when load rises [8]. *Design consequence:* anything that adds a parallel
task (monitoring numbers, dismissing suggestions, fixing underlines,
operating the tool) takes load from composing.

**F25 · Drafting creates ideas, not only records them. — Moderate.**
Galbraith's dual-process model: spontaneous text production
("knowledge-constituting") produces discovery, while top-down planning
produces another kind of content [5]. Discovery was linked to spontaneous
sentence production and to global revision [6]. This supports a tool that
makes it easy to keep going (fast, quiet, forgiving) and easy to revise
later (history, export).

**F26 · "Rough draft first" is not proven better than polishing as you
go; outlining is. — Moderate.**
Kellogg's experiments: a rough-draft strategy changed *when* writers
reviewed but did not improve quality or efficiency; a written or mental
outline did improve quality [3]. The "silence the inner editor" advice
(freewriting [86]) is *Opinion* as a general rule; it may help writers who
block, and Boice's work with blocked academics supports scheduled,
regular writing (see F30). *Consequence:* typewriter mode (no backspace) is a
legitimate optional exercise, not a default.

**F27 · Word processors help developing writers. — Strong (for students).**
Meta-analyses of school writing find word processing improves quality
(effect size ~0.5) [14]. Kellogg found word processing changed planning and
reviewing without changing quality or fluency for adults (abstract) [3].
For skilled adults, the editor matters less than the absence of competing
tasks.

### 2.4 Flow, ritual and the session

**F28 · Starting friction and plans. — Strong (plans) / Opinion (rituals).**
If-then plans ("when I sit down after breakfast, I write") raise goal
attainment (meta-analysis, d ≈ 0.65) [60]. Writers' rituals (same place, same
time, same tool) are craft advice [58, 59] (*Opinion*), consistent with
habit research. Fast start-up (rule 5) removes a real point where people
drift away (*Opinion*, plausible).

**F29 · Resuming: give the exact place and its context. — Moderate.**
Resumption takes longer when cues are missing; preserving the task display
and letting people encode where they were shortens the restart [43, 44].
People tend to go back to unfinished tasks (Ovsiankina effect: robust in a
2025 meta-analysis), but unfinished tasks are *not* better remembered
(Zeigarnik effect: not general) [61]. Hemingway's "stop mid-sentence" works
as a *resume cue*, not as a memory trick.

**F30 · Regular, scheduled sessions produce more writing. — Moderate.**
Boice: blocked academics who wrote on a schedule produced more pages and more
creative ideas than those who wrote "when inspired" [57]; across his faculty
studies, brief daily sessions beat binges [58] (small samples, older work).

**F31 · Flow needs clear goals, immediate feedback and no
interruption. — Moderate (theory, survey research).**
Csikszentmihalyi's conditions [62] map onto: a goal for the session, text
appearing at once, nothing else on screen. Flow itself is hard to measure;
treat as framing.

**F32 · Breaks. — Moderate.**
Micro-breaks raise vigour and reduce fatigue (d ≈ 0.35), with no reliable
performance gain for demanding tasks (22 samples) [74]. A walk before or
during a break raised creative idea generation in four experiments [75].
The 20-20-20 eye rule has only short-term, weak support [35]. Nothing here
justifies break *reminders*, which are interruptions (F14).

**F33 · Time of day. — Moderate (synchrony), Weak (insight).**
People do better on analytic tasks at their preferred time of day [72];
insight problems were solved better at the *non*-preferred time in one
large study [73]. No product rule follows except: do not prescribe a time.

**F34 · Sound. — Weak.**
Irrelevant speech and secondary tasks reduce writing fluency [77]. The
"moderate noise boosts creativity" result [76] is a single paper with no
direct replication found. Carrel's silence is consistent with the evidence;
keystroke sounds have no evidence either way (*Opinion*).

**F35 · Posture and keyboard. — Moderate (posture), Opinion (keyboard).**
Laptops force a low screen or a high keyboard; a raised screen with a
separate keyboard reduced neck load and perceived strain [78]. We found no
good evidence that key type (mechanical, travel) changes writing. Practical
consequence: an external USB keyboard must work fully, with the chosen
layout, so a writer can raise the laptop.

### 2.5 Dedicated devices and minimal editors

**F36 · No controlled studies of Freewrite, iA Writer-style editors or
"distraction-free" modes. — none.**
Searches found product reviews and writers' accounts only. Classroom studies
of AlphaSmart-type devices are small: students wrote more words, with mixed
quality effects [82] (*Weak*). Writers' reports (for example George R. R.
Martin's use of WordStar 4.0 on an offline DOS machine) are *Opinion*: they
show a demand, not an effect.

**F37 · Digital minimalism. — Opinion (book) / Strong (adjacent RCT).**
Newport's "digital minimalism" [80] is an argument, not a study. The nearest
strong evidence: deactivating Facebook for four weeks raised subjective
well-being and freed about an hour a day (large RCT) [81]. The
mechanism Carrel relies on is the better-tested one in F14–F15.

### 2.6 What to avoid: engagement mechanics

**F38 · Expected tangible rewards undermine intrinsic motivation;
informational feedback does not. — Strong.**
Meta-analysis of 128 experiments: expected, task-contingent tangible rewards
lowered free-choice interest (d ≈ −0.34 for engagement-contingent; larger for
completion-contingent); positive verbal feedback *raised* it [63]. Classic
study: expected awards reduced later drawing by children [64].
Self-determination theory: rewards felt as *controlling* hurt; feedback felt
as *informational* helps [65]. Intrinsic and extrinsic incentives can work
together for quantity, but intrinsic motivation predicts quality [71].

**F39 · Extrinsic framing hurts creative writing. — Weak–Moderate.**
Writers asked to rank extrinsic reasons for writing before composing a poem
wrote less creative poems than those primed with intrinsic reasons [66].

**F40 · Gamification: mixed, and riskiest for self-chosen work. —
Moderate.**
In education, gamification has small positive effects on average [68]. But
in a 16-week course, badges and a leaderboard *lowered* intrinsic
motivation, satisfaction and exam scores [67]; points raised quantity of
work but not intrinsic motivation [69]. **Streaks**: intact streaks raise
engagement, but showing a *broken* streak lowers later engagement, more when
people blame themselves (7 studies) [70]. For a tool people use by their own
choice for creative work, the downside risk dominates.

**F41 · Toasts, badges, pop-ups. — Strong by inference.**
These are interruptions (F14) and motion onsets (F17) and, when they praise
output, controlling rewards (F38). No study tests "toasts in writing apps"
directly.

---

## 3. Design rules for Carrel

Each rule is testable. Strength follows the weakest needed link.
"F" numbers point to section 2.

### Typography and display

- **D1 · Size text by physical size, not pixels.** Derive px from the
  panel's real DPI (EDID) so one setting gives the same physical size on
  every laptop. At the default size, the lower-case x-height must be
  0.25°–0.3° of visual angle at 55 cm (2.4–2.9 mm); at the smallest size the
  user can choose, never below 0.2° (1.9 mm). Test: measure the height of
  "x" on screen with a ruler on the lowest-ppi and highest-ppi tier 1
  laptops. — *Strong* (F1).
- **D2 · Keep the column at 55–80 characters** and never wider than 80, at
  every font size the user can choose. Test: count characters of a
  representative paragraph at 16 and 24 px. — *Moderate* (F2).
- **D3 · Keep line height between 1.5 and 1.7.** — *Moderate* (F3).
- **D4 · Default font: serif or sans, never mono.** Mono stays an option. —
  *Moderate* (F4, F5).
- **D5 · Body text contrast ≥ 7:1, any text the writer must read ≥ 4.5:1,
  non-text UI marks (caret, hairline, focus outline) ≥ 3:1.** Test with the
  WCAG formula on the final palette. — *Strong* (standard [37], F7).
- **D6 · Paper is the default theme.** Night is for dark rooms and for
  readers who prefer it. Do not describe Night as healthier or
  battery-saving. — *Strong* (F7, F8).
- **D7 · Brightness keys work in every screen, and the device never forces
  full brightness.** — *Moderate* (F8, F9).
- **D8 · No colour-temperature or "night light" feature.** — *Moderate*
  (F10).

### Attention

- **D9 · Nothing on the editor screen changes unless the writer acts,
  except the battery warnings.** No blinking caret, no animated counters, no
  clock seconds. Test: record the idle editor for 60 s; frames must be
  identical. — *Strong* (F14, F17).
- **D10 · No suggestions while typing**: no autocomplete, no autocorrect, no
  grammar hints. — *Weak–Moderate* (F22, rule 9).
- **D11 · Spellcheck is off by default and, when on, marks without moving
  text or showing pop-ups.** Offer a separate "check spelling" pass. —
  *Weak* (F22).
- **D12 · Dimmed context must stay legible:** focus mode dims other
  paragraphs to no less than 3:1 contrast (Paper ≈ 50 % ink, Night ≈ 40 %),
  and better to the 4.5:1 dim level. — *Weak* (F18, F20) on top of
  *Strong* legibility thresholds.
- **D13 · Never hide the writer's own text by default.** — *Weak* (F20).

### Goals and numbers

- **D14 · One daily goal, set by the writer, shown only on request (F1) or
  as the hairline.** — *Strong* for goals and monitoring (F21).
- **D15 · Record progress:** keep a daily word log (from History) that the
  writer can view; do not push it. — *Strong* (F21, recorded monitoring).
- **D16 · No streaks, badges, points, levels, confetti, praise messages or
  "you reached your goal" banners.** Reaching the goal changes nothing but
  the bar being full. — *Strong* (F38) / *Moderate* (F40).
- **D17 · Show progress, not judgement:** no red, no "behind" states, no
  comparison with past days on the editor screen. — *Moderate* (F38, F40).

### Session

- **D18 · Resume at the exact caret position** in the last file, with the
  caret's paragraph and the one before it visible, after resume and after
  cold boot. Test: write, power off, boot, check caret and view. —
  *Moderate* (F29).
- **D19 · No break or posture reminders.** — *Moderate* (F14, F32).
- **D20 · External USB keyboards work with the selected layout, in the first
  run too.** — *Moderate* (F35).
- **D21 · Keep print-ready export (PDF) easy for the revision pass.** —
  *Weak* (F12).

### Copy and claims

- **D22 · Do not claim effects that the evidence does not support** in
  docs, the website or About: "easier on the eyes", "better sleep",
  "phone-free brain boost", "monospace stops perfectionism", "proven to make
  you write more". Claims that are supported: fewer interruptions, quiet
  screen, fast start, nothing to switch to. — *Strong* (sections 2.1–2.6).

---

## 4. Evidence map for the rules

| Rule | Main findings | Strength |
|---|---|---|
| D1 | F1 | Strong |
| D2 | F2 | Moderate |
| D3 | F3 | Moderate |
| D4 | F4, F5 | Moderate |
| D5 | F7, WCAG | Strong |
| D6 | F7, F8 | Strong |
| D7 | F8, F9 | Moderate |
| D8 | F10 | Moderate |
| D9 | F14, F17 | Strong |
| D10 | F22, F24 | Weak–Moderate |
| D11 | F22 | Weak |
| D12 | F18, F20, WCAG | Weak (dimming) / Strong (threshold) |
| D13 | F20 | Weak |
| D14 | F21 | Strong |
| D15 | F21 | Strong |
| D16 | F38, F40 | Strong / Moderate |
| D17 | F38, F40 | Moderate |
| D18 | F29 | Moderate |
| D19 | F14, F32 | Moderate |
| D20 | F35 | Moderate |
| D21 | F12 | Weak |
| D22 | all | Strong |

---

## 5. Where the current Carrel design agrees or disagrees with the evidence

References are to `00-rules.md` (rules) and `03-ui.md` (UI).

### Agrees

| Design point | Evidence |
|---|---|
| One app owns the screen; no app switching (rule 1) | F14, F15 — Strong/Moderate. The best-supported part of Carrel. |
| Network off unless the user syncs (rule 2) | F15 (blocking self-interruption) — Moderate. |
| Silence: no pop-ups, toasts, badges, sounds, update prompts (rule 6) | F14, F17, F38, F41 — Strong. |
| Fast to the cursor (rule 5) | F28, F29 — Opinion/Moderate. |
| No AI writes for the user (rule 9) | F22 (predictive text makes writing more predictable) — Weak–Moderate. |
| Paper theme with near-black ink, ~14:1 contrast | F7 — Strong. |
| No pure black/white; warm off-white | F9, F13 — Weak/Opinion. Harmless. |
| Column 60–75 characters | F2 — Moderate. |
| Line height 1.6 | F3 — Moderate. |
| Three fonts including serif and sans | F4 — Moderate. |
| No animation except ≤150 ms fade; no caret animation | F17 — Strong. |
| No keystroke sounds | F34 — Weak/Opinion. |
| Bottom line can be hidden (F1); Hairline option hides numbers | F21 — Moderate (lets writers who find numbers stressful remove them). |
| Hairline is "full, not over-full" when the goal is reached | F38, F40 — no celebration, no overshoot score. Consistent. |
| Spellcheck off by default | F22 — Weak. |
| Typewriter mode (no backspace) is optional, not default | F26 — Moderate (no proven quality benefit). |
| Focus mode is off by default | F18 — Opinion. |
| History with word-count delta | F21 (recorded monitoring) — Strong. |
| Update shows only in Settings | F14, F41 — Strong. |
| Battery appears briefly at 10 % and 5 % even when hidden | Necessary interruption to protect text (rule 3). Acceptable. |
| PDF export | F12 — Weak. |
| Brightness keys on hardware checklist (`04-hardware.md`) | F8, F9 — Moderate. |
| No clock by default (clock optional) | Opinion. Fine. |

### Disagrees or needs a change

1. **Body text size is given in px (19 px, range 16–24).**
   On a 13.3" 1920×1080 panel (≈166 ppi), 19 px is ≈2.9 mm per em, and a
   book serif's x-height (~0.45–0.5 em) is ≈1.3–1.45 mm: about 0.14–0.15° at
   55 cm, *below* the ~0.2° critical print size (F1, Strong). On a 1366×768
   14" panel (≈112 ppi) the same setting is ≈0.21–0.23°, at the floor; only
   on a ~96 ppi panel does it reach ≈0.25°. Fix: define the default and range
   in physical units (or visual angle) and convert with the panel DPI; a
   reasonable default is an x-height of 0.25°–0.3° (2.4–2.9 mm at 55 cm),
   with 0.2° as the smallest allowed size (D1).
2. **Focus mode draws other paragraphs at 35 % opacity.**
   Measured contrast: Paper 2.1:1, Night 2.6:1 — below the 3:1 floor for
   any text. Writers re-read prior text constantly while drafting (F18, F20).
   Fix: dim to the existing "dim text" level or a contrast floor of 3:1
   minimum, 4.5:1 preferred (D12). For Paper, 4.5:1 needs ≈65 % ink; for
   Night, ≈55 %.
3. **Dim text = ink at 55 % in Paper gives 3.5:1** (Night: 4.6:1, passes).
   Markdown markers (`**`) can be low-contrast on purpose, but any text the
   writer must *read* (bottom-line numbers, file dates and sizes, History
   deltas, Settings hints, citation markers) needs 4.5:1 (D5). Fix: Paper
   dim ≈65 % ink (4.7:1), or two dim levels (markers vs readable labels).
4. **Selected row uses the accent colour.** If the row is *filled* with the
   accent, Paper text on terracotta is 3.6:1 (ink) or 3.9:1 (paper), below
   4.5:1. Night is fine with dark text (8.2:1) but fails with light text
   (1.5:1). Fix: mark the selected row with an accent bar or outline (≥3:1
   as a non-text mark, which terracotta meets at 3.8:1 and amber at 8.1:1),
   and keep row text in ink.

### No evidence either way (keep, but do not claim it is evidence-based)

- Typewriter scrolling (F19).
- Focus mode itself (F18).
- Warm tint of the background (F13).
- Choice of terracotta/amber accent.
- Daily goal shown as numbers by default rather than the hairline (F21:
  monitoring helps; no study of live counters while writing).
- Night theme as one of two themes: fine as comfort; just not as a health
  feature (F7, F8, D6). The spec does not name the default theme; it should
  be Paper (D6).

---

## 6. References

Strength of each source is given in the findings, not here.

1. Hayes, J. R., & Flower, L. S. (1980). Identifying the organization of writing processes. In L. W. Gregg & E. R. Steinberg (Eds.), *Cognitive Processes in Writing* (pp. 3–30). Erlbaum.
2. Hayes, J. R. (2012). Modeling and remodeling writing. *Written Communication*, 29(3), 369–388. https://doi.org/10.1177/0741088312451260
3. Kellogg, R. T. (1988). Attentional overload and writing performance: Effects of rough draft and outline strategies. *Journal of Experimental Psychology: Learning, Memory, and Cognition*, 14(2), 355–365. https://scholarsmine.mst.edu/psysci_facwork/284 — also Kellogg, R. T., & Mueller, S. (1993), Performance amplification and process restructuring in computer-based writing, *International Journal of Man-Machine Studies*, 39, 33–49. https://scholarsmine.mst.edu/psysci_facwork/276
4. Kellogg, R. T. (2008). Training writing skills: A cognitive developmental perspective. *Journal of Writing Research*, 1(1), 1–26. https://doi.org/10.17239/jowr-2008.01.01.1
5. Galbraith, D., & Baaijen, V. M. (2018). The work of writing: Raiding the inarticulate. *Educational Psychologist*, 53(4), 238–257. https://eprints.soton.ac.uk/425716
6. Baaijen, V. M., & Galbraith, D. (2018). Discovery through writing: Relationships with writing processes and text quality. *Cognition and Instruction*, 36(3), 199–223. https://eprints.soton.ac.uk/421401
7. Bereiter, C., & Scardamalia, M. (1987). *The Psychology of Written Composition*. Erlbaum.
8. Chenoweth, N. A., & Hayes, J. R. (2001). Fluency in writing: Generating text in L1 and L2. *Written Communication*, 18(1), 80–98. https://doi.org/10.1177/0741088301018001004
9. Olive, T., & Piolat, A. (2002). Suppressing visual feedback in written composition: Effects on processing demands and coordination of the writing processes. *International Journal of Psychology*, 37(4), 209–218. https://doi.org/10.1080/00207590244000089
10. Blau, S. (1983). Invisible writing: Investigating cognitive processes in composition. *College Composition and Communication*, 34(3), 297–312. https://doi.org/10.2307/358261
11. Johansson, R., Wengelin, Å., Johansson, V., & Holmqvist, K. (2010). Looking at the keyboard or the monitor: Relationship with text production processes. *Reading and Writing*, 23(7), 835–851. https://doi.org/10.1007/s11145-009-9189-3
12. Arnold, K. C., Chauncey, K., & Gajos, K. Z. (2020). Predictive text encourages predictable writing. *Proc. IUI '20*, 128–138. https://doi.org/10.1145/3377325.3377523 (PDF: https://kgajos.seas.harvard.edu/papers/arnold20predictive.pdf)
13. Figueredo, L., & Varnhagen, C. K. (2005). Didn't you run the spell checker? Effects of type of spelling error and use of a spell checker on perceptions of the author. *Reading Psychology*, 26(4–5), 441–458.
14. Graham, S., & Perin, D. (2007). A meta-analysis of writing instruction for adolescent students. *Journal of Educational Psychology*, 99(3), 445–476. https://doi.org/10.1037/0022-0663.99.3.445
15. Dyson, M. C., & Kipping, G. J. (1998). The effects of line length and method of movement on patterns of reading from screen. *Visible Language*, 32(2), 150–181. https://journals.uc.edu/index.php/vl/article/view/5671
16. Dyson, M. C. (2004). How physical text layout affects reading from screen. *Behaviour & Information Technology*, 23(6), 377–393. https://doi.org/10.1080/01449290410001715714
17. Rello, L., Pielot, M., & Marcos, M.-C. (2016). Make it big! The effect of font size and line spacing on online readability. *Proc. CHI '16*, 3637–3648. https://doi.org/10.1145/2858036.2858204
18. Legge, G. E., & Bigelow, C. A. (2011). Does print size matter for reading? A review of findings from vision science and typography. *Journal of Vision*, 11(5):8. https://doi.org/10.1167/11.5.8
19. Mansfield, J. S., Legge, G. E., & Bane, M. C. (1996). Psychophysics of reading XV: Font effects in normal and low vision. *Investigative Ophthalmology & Visual Science*, 37(8), 1492–1501. https://legge.dl8.umn.edu/sites/legge.psych.umn.edu/files/files/media/mansfield96_psychophysics_of_reading_xv-_font_effects_in_normal_and_low_vision.pdf
20. Arditi, A., & Cho, J. (2005). Serifs and font legibility. *Vision Research*, 45(23), 2926–2933. https://doi.org/10.1016/j.visres.2005.06.013
21. Chung, S. T. L. (2004). Reading speed benefits from increased vertical word spacing in normal peripheral vision. *Optometry and Vision Science*, 81(7), 525–535. https://doi.org/10.1097/00006324-200407000-00014
22. Piepenbrock, C., Mayr, S., Mund, I., & Buchner, A. (2013). Positive display polarity is advantageous for both younger and older adults. *Ergonomics*, 56(7), 1116–1124. https://doi.org/10.1080/00140139.2013.790485
23. Piepenbrock, C., Mayr, S., & Buchner, A. (2014). Smaller pupil size and better proofreading performance with positive than with negative polarity displays. *Ergonomics*, 57(11), 1670–1677. https://doi.org/10.1080/00140139.2014.948496
24. Piepenbrock, C., Mayr, S., & Buchner, A. (2014). Positive display polarity is particularly advantageous for small character sizes: Implications for display design. *Human Factors*, 56(5), 942–951. https://doi.org/10.1177/0018720813515509
25. Buchner, A., & Baumgartner, N. (2007). Text–background polarity affects performance irrespective of ambient illumination and colour contrast. *Ergonomics*, 50(7), 1036–1063. https://doi.org/10.1080/00140130701306413
26. Dobres, J., Chahine, N., & Reimer, B. (2017). Effects of ambient illumination, contrast polarity, and letter size on text legibility under glance-like reading. *Applied Ergonomics*, 60, 68–73. https://pubmed.ncbi.nlm.nih.gov/28166901/
27. Dash, P., & Hu, Y. C. (2021). How much battery does dark mode save? An accurate OLED display power profiler for modern smartphones. *Proc. MobiSys '21*. Summary: https://www.purdue.edu/newsroom/releases/2021/Q3/dark-mode-may-not-save-your-phones-battery-life-as-much-as-you-think,-but-there-are-a-few-silver-linings.html
28. Chang, A.-M., Aeschbach, D., Duffy, J. F., & Czeisler, C. A. (2015). Evening use of light-emitting eReaders negatively affects sleep, circadian timing, and next-morning alertness. *PNAS*, 112(4), 1232–1237. https://doi.org/10.1073/pnas.1418490112
29. Duraccio, K. M., Zaugg, K. K., Blackburn, R. C., & Jensen, C. D. (2021). Does iPhone night shift mitigate negative effects of smartphone use on sleep outcomes in emerging adults? *Sleep Health*, 7(4), 478–484. https://doi.org/10.1016/j.sleh.2021.03.005
30. Singh, S., Keller, P. R., Busija, L., et al. (2023). Blue-light filtering spectacle lenses for visual performance, sleep, and macular health in adults. *Cochrane Database of Systematic Reviews*, CD013244. https://doi.org/10.1002/14651858.CD013244.pub2
31. Benedetto, S., Drai-Zerbib, V., Pedrotti, M., Tissier, G., & Baccino, T. (2013). E-readers and visual fatigue. *PLoS ONE*, 8(12), e83676. https://doi.org/10.1371/journal.pone.0083676
32. Benedetto, S., Carbone, A., Drai-Zerbib, V., Pedrotti, M., & Baccino, T. (2014). Effects of luminance and illuminance on visual fatigue and arousal during digital reading. *Computers in Human Behavior*, 41, 112–119. https://doi.org/10.1016/j.chb.2014.09.023
33. Kretzschmar, F., Pleimling, D., Hosemann, J., et al. (2013). Subjective impressions do not mirror online reading effort: Concurrent EEG-eyetracking evidence from the reading of books and digital media. *PLoS ONE*, 8(2), e56178. https://doi.org/10.1371/journal.pone.0056178
34. Sheppard, A. L., & Wolffsohn, J. S. (2018). Digital eye strain: Prevalence, measurement and amelioration. *BMJ Open Ophthalmology*, 3, e000146. https://doi.org/10.1136/bmjophth-2018-000146
35. Talens-Estarelles, C., Cerviño, A., García-Lázaro, S., Fogelton, A., Sheppard, A., & Wolffsohn, J. S. (2023). The effects of breaks on digital eye strain, dry eye and binocular vision: Testing the 20-20-20 rule. *Contact Lens and Anterior Eye*, 46(2), 101744. https://doi.org/10.1016/j.clae.2022.101744
36. Delgado, P., Vargas, C., Ackerman, R., & Salmerón, L. (2018). Don't throw away your printed books: A meta-analysis on the effects of reading media on reading comprehension. *Educational Research Review*, 25, 23–38. https://doi.org/10.1016/j.edurev.2018.09.003
37. W3C (2023). *Web Content Accessibility Guidelines (WCAG) 2.2* — SC 1.4.3, 1.4.6, 1.4.8, 1.4.11, 1.4.12, 2.3.3. https://www.w3.org/TR/WCAG22/
38. ISO 9241-303:2011. *Ergonomics of human-system interaction — Part 303: Requirements for electronic visual displays.* https://www.iso.org/standard/57489.html
39. Monsell, S. (2003). Task switching. *Trends in Cognitive Sciences*, 7(3), 134–140. https://doi.org/10.1016/S1364-6613(03)00028-7
40. Altmann, E. M., Trafton, J. G., & Hambrick, D. Z. (2014). Momentary interruptions can derail the train of thought. *Journal of Experimental Psychology: General*, 143(1), 215–226 (online 2013). https://doi.org/10.1037/a0030986
41. Mark, G., Gudith, D., & Klocke, U. (2008). The cost of interrupted work: More speed and stress. *Proc. CHI '08*, 107–110. https://doi.org/10.1145/1357054.1357072
42. Altmann, E. M., & Trafton, J. G. (2002). Memory for goals: An activation-based model. *Cognitive Science*, 26(1), 39–83. https://doi.org/10.1207/s15516709cog2601_2
43. Trafton, J. G., Altmann, E. M., Brock, D. P., & Mintz, F. E. (2003). Preparing to resume an interrupted task: Effects of prospective goal encoding and retrospective rehearsal. *International Journal of Human-Computer Studies*, 58(5), 583–603. https://interruptions.net/literature/Trafton-IJHCS03.pdf
44. Altmann, E. M., & Trafton, J. G. (2004). Task interruption: Resumption lag and the role of cues. *Proc. 26th Annual Conference of the Cognitive Science Society*.
45. Kushlev, K., Proulx, J., & Dunn, E. W. (2016). "Silence your phones": Smartphone notifications increase inattention and hyperactivity symptoms. *Proc. CHI '16*, 1011–1020. https://www.interruptions.net/literature/Kushlev-CHI16.pdf
46. Stothart, C., Mitchum, A., & Yehnert, C. (2015). The attentional cost of receiving a cell phone notification. *Journal of Experimental Psychology: Human Perception and Performance*, 41(4), 893–897. https://doi.org/10.1037/xhp0000100
47. Ward, A. F., Duke, K., Gneezy, A., & Bos, M. W. (2017). Brain drain: The mere presence of one's own smartphone reduces available cognitive capacity. *Journal of the Association for Consumer Research*, 2(2), 140–154. https://doi.org/10.1086/691462
48. Ruiz Pardo, A. C., & Minda, J. P. (2022). Reexamining the "brain drain" effect: A replication of Ward et al. (2017). *Acta Psychologica*, 230, 103717. https://pubmed.ncbi.nlm.nih.gov/36007374/
49. Mark, G., Czerwinski, M., & Iqbal, S. T. (2018). Effects of individual differences in blocking workplace distractions. *Proc. CHI '18*. https://www.microsoft.com/en-us/research/publication/effects-individual-differences-blocking-workplace-distractions/
50. Dabbish, L., Mark, G., & González, V. M. (2011). Why do I keep interrupting myself? Environment, habit and self-interruption. *Proc. CHI '11*, 3127–3130. https://doi.org/10.1145/1978942.1979405
51. Abrams, R. A., & Christ, S. E. (2003). Motion onset captures attention. *Psychological Science*, 14(5), 427–432. https://doi.org/10.1111/1467-9280.01458
52. Harkin, B., Webb, T. L., Chang, B. P. I., et al. (2016). Does monitoring goal progress promote goal attainment? A meta-analysis of the experimental evidence. *Psychological Bulletin*, 142(2), 198–229. https://doi.org/10.1037/bul0000025 (preprint: https://eprints.whiterose.ac.uk/87431/)
53. Koo, M., & Fishbach, A. (2012). The small-area hypothesis: Effects of progress monitoring on goal adherence. *Journal of Consumer Research*, 39(3), 493–509. https://doi.org/10.1086/663827
54. Kivetz, R., Urminsky, O., & Zheng, Y. (2006). The goal-gradient hypothesis resurrected. *Journal of Marketing Research*, 43(1), 39–58. https://doi.org/10.1509/jmkr.43.1.39
55. Villar, A., Callegaro, M., & Yang, Y. (2013). Where am I? A meta-analysis of experiments on the effects of progress indicators for web surveys. *Social Science Computer Review*, 31(6), 744–762. https://doi.org/10.1177/0894439313497468
56. Locke, E. A., & Latham, G. P. (2002). Building a practically useful theory of goal setting and task motivation. *American Psychologist*, 57(9), 705–717. https://doi.org/10.1037/0003-066X.57.9.705
57. Boice, R. (1983). Contingency management in writing and the appearance of creative ideas: Implications for the treatment of writing blocks. *Behaviour Research and Therapy*, 21(5), 537–543.
58. Boice, R. (1990). *Professors as Writers: A Self-Help Guide to Productive Writing*. New Forums Press.
59. Silvia, P. J. (2019). *How to Write a Lot* (2nd ed.). APA.
60. Gollwitzer, P. M., & Sheeran, P. (2006). Implementation intentions and goal achievement: A meta-analysis of effects and processes. *Advances in Experimental Social Psychology*, 38, 69–119. https://doi.org/10.1016/S0065-2601(06)38002-1
61. Ghibellini, R., & Meier, B. (2025). Interruption, recall and resumption: A meta-analysis of the Zeigarnik and Ovsiankina effects. *Humanities and Social Sciences Communications*, 12. https://doi.org/10.1057/s41599-025-05000-w
62. Csikszentmihalyi, M. (1990). *Flow: The Psychology of Optimal Experience*. Harper & Row.
63. Deci, E. L., Koestner, R., & Ryan, R. M. (1999). A meta-analytic review of experiments examining the effects of extrinsic rewards on intrinsic motivation. *Psychological Bulletin*, 125(6), 627–668. https://doi.org/10.1037/0033-2909.125.6.627
64. Lepper, M. R., Greene, D., & Nisbett, R. E. (1973). Undermining children's intrinsic interest with extrinsic reward: A test of the "overjustification" hypothesis. *Journal of Personality and Social Psychology*, 28(1), 129–137. https://doi.org/10.1037/h0035519
65. Ryan, R. M., & Deci, E. L. (2000). Self-determination theory and the facilitation of intrinsic motivation, social development, and well-being. *American Psychologist*, 55(1), 68–78. https://doi.org/10.1037/0003-066X.55.1.68
66. Amabile, T. M. (1985). Motivation and creativity: Effects of motivational orientation on creative writers. *Journal of Personality and Social Psychology*, 48(2), 393–399. https://doi.org/10.1037/0022-3514.48.2.393
67. Hanus, M. D., & Fox, J. (2015). Assessing the effects of gamification in the classroom: A longitudinal study on intrinsic motivation, social comparison, satisfaction, effort, and academic performance. *Computers & Education*, 80, 152–161. https://doi.org/10.1016/j.compedu.2014.08.019
68. Sailer, M., & Homner, L. (2020). The gamification of learning: A meta-analysis. *Educational Psychology Review*, 32, 77–112. https://doi.org/10.1007/s10648-019-09498-w
69. Mekler, E. D., Brühlmann, F., Tuch, A. N., & Opwis, K. (2017). Towards understanding the effects of individual gamification elements on intrinsic motivation and performance. *Computers in Human Behavior*, 71, 525–534. https://doi.org/10.1016/j.chb.2015.08.048
70. Silverman, J., & Barasch, A. (2023). On or off track: How (broken) streaks affect consumer decisions. *Journal of Consumer Research*, 49(6), 1095–1117. https://doi.org/10.1093/jcr/ucac029
71. Cerasoli, C. P., Nicklin, J. M., & Ford, M. T. (2014). Intrinsic motivation and extrinsic incentives jointly predict performance: A 40-year meta-analysis. *Psychological Bulletin*, 140(4), 980–1008. https://doi.org/10.1037/a0035661
72. May, C. P., Hasher, L., & Stoltzfus, E. R. (1993). Optimal time of day and the magnitude of age differences in memory. *Psychological Science*, 4(5), 326–330. https://doi.org/10.1111/j.1467-9280.1993.tb00573.x
73. Wieth, M. B., & Zacks, R. T. (2011). Time of day effects on problem solving: When the non-optimal is optimal. *Thinking & Reasoning*, 17(4), 387–401. https://doi.org/10.1080/13546783.2011.625663
74. Albulescu, P., Macsinga, I., Rusu, A., et al. (2022). "Give me a break!" A systematic review and meta-analysis on the efficacy of micro-breaks for increasing well-being and performance. *PLoS ONE*, 17(8), e0272460. https://doi.org/10.1371/journal.pone.0272460
75. Oppezzo, M., & Schwartz, D. L. (2014). Give your ideas some legs: The positive effect of walking on creative thinking. *Journal of Experimental Psychology: Learning, Memory, and Cognition*, 40(4), 1142–1152. https://doi.org/10.1037/a0036577
76. Mehta, R., Zhu, R., & Cheema, A. (2012). Is noise always bad? Exploring the effects of ambient noise on creative cognition. *Journal of Consumer Research*, 39(4), 784–799. https://doi.org/10.1086/665048
77. Ransdell, S., Levy, C. M., & Kellogg, R. T. (2002). The structure of writing processes as revealed by secondary task demands. *L1 – Educational Studies in Language and Literature*, 2(2), 141–163. https://doi.org/10.1023/A:1020851300668
78. Berkhout, A. L., Hendriksson-Larsén, K., & Bongers, P. (2004). The effect of using a laptopstation compared to using a standard laptop PC on the cervical spine torque, perceived strain and productivity. *Applied Ergonomics*, 35(2), 147–152. https://doi.org/10.1016/j.apergo.2003.11.008
79. Fitz, N., Kushlev, K., Jagannathan, R., Lewis, T., Paliwal, D., & Ariely, D. (2019). Batching smartphone notifications can improve well-being. *Computers in Human Behavior*, 101, 84–94. https://doi.org/10.1016/j.chb.2019.07.016
80. Newport, C. (2019). *Digital Minimalism: Choosing a Focused Life in a Noisy World*. Portfolio.
81. Allcott, H., Braghieri, L., Eichmeyer, S., & Gentzkow, M. (2020). The welfare effects of social media. *American Economic Review*, 110(3), 629–676. https://doi.org/10.1257/aer.20190658
82. Classroom AlphaSmart study (Hamline University capstone): *Using AlphaSmarts to improve the quantity and quality of writing of fifth grade English language learners*. https://digitalcommons.hamline.edu/hse_all/365
83. Meyer, A., Frederick, S., Burnham, T. C., et al. (2015). Disfluent fonts don't help people solve math problems. *Journal of Experimental Psychology: General*, 144(2), e16–e30. https://doi.org/10.1037/xge0000049
84. Bringhurst, R. (2004). *The Elements of Typographic Style* (3rd ed.). Hartley & Marks.
85. Iqbal, S. T., & Horvitz, E. (2007). Disruption and recovery of computing tasks: Field study, analysis, and directions. *Proc. CHI '07*, 677–686. https://doi.org/10.1145/1240624.1240730
86. Elbow, P. (1973). *Writing Without Teachers*. Oxford University Press.

**Method note.** Sources were found with web searches of journal databases,
publisher pages, PubMed and author pages (November 2026). For several papers
only the abstract or a reliable secondary summary was read; those numbers
are marked in the text. Contrast ratios in section 5 were computed with the
WCAG 2.x relative-luminance formula from the hex values in `03-ui.md`,
blending ink over the background at the stated opacity.
