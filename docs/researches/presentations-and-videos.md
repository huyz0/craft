# Presentations and videos

Prioritized media for the requested AI software factory study. Accessed 8 November 2026. **Transcript reviewed**, **slide text reviewed**, **chapter outline reviewed**, and **listing only** describe the evidence actually inspected. They do not imply that the full recording was watched.

## 1. Daniel Sogl: Spec Driven Development, The End of Vibe Coding

- [Published recording and transcript](https://gitnation.com/contents/spec-driven-development-the-end-of-vibe-coding), AI Coding Summit, 26 February 2026.
- [June 2026 presentation deck](https://speakerdeck.com/danielsogl/spec-driven-development-vibekodecon-munich-2026).
- [Speaker's canonical talk page](https://danielsogl.de/en/speaking/spec-driven-development).
- Evidence: **published transcript and slide text reviewed**. Sources S26 and S27.

The transcript connects SDD with behavior-driven and test-driven development and discusses the overhead of a full spec workflow for a small repair. At **07:09**, the discussion compares OpenSpec and Spec Kit; **09:27** introduces the constitution and development phases; **11:36** discusses test-first implementation.

The June deck adds concrete task dependencies, validation commands, plan artifacts, and a discussion of team-level spec drift. Slides **9–10** cover spec structure; **13–16** cover phases and repository rules; **19–23** show planning, tasks, and consistency checks; **24–25** discuss limitations and when the process is worth its overhead.

Implication for our proposal: link tasks to acceptance conditions and validation evidence, and provide a shorter route for small changes. The presentation's quantitative productivity and security claims were not independently validated here and are not used as pilot forecasts. Commands in a historical deck should be checked against current tool docs.

## 2. Spec Stacking & Software Factories

- [Maven lesson and chapter outline](https://maven.com/p/9db11e/spec-stacking-software-factories), 29 May 2026, listed as 45 minutes.
- Evidence: **published chapter outline and lesson description reviewed**. Recording itself not reviewed. Source S31.

Useful chapter pointers:

| Chapter | Why inspect it |
| --- | --- |
| 05:01 and 09:17 | Composable specifications and the assembly demonstration |
| 14:14 | Expressing behavior through BDD |
| 18:30 and 22:10 | Risk and gated delivery workflow |
| 30:39 | Stop conditions and evaluator gaming |
| 35:57 | Keeping documentation and software aligned |

The published outline makes this a useful follow-up for the architecture-to-verification connection. It does not establish measured reliability or performance of the proposed techniques.

## 3. Jakub Kuzimski: Stop Making LLMs Deterministic, A Spec-Driven Software Factory

- [Recording](https://youtu.be/AA12Ahf5_m0).
- [Indexed talk and chapter listing](https://confpad.io/conferences/agent-conf-2026/stop-making-llms-deterministic-a-spec-driven-software-factory), Agent Conf, 17–18 September 2026, listed as 23 minutes.
- Evidence: **third-party listing and chapters reviewed**. The listing explicitly labels its summary AI-written. No primary transcript reviewed. Source S32.

The most relevant listed chapters are **06:40** for the multi-agent pipeline, **11:03–12:49** for graph-shaped specifications, **16:42** for generated artifacts and validation, and **19:03** for reviews exposing missing decisions.

Use these as navigation pointers for a later primary review. The index's cost, duration, and generated-code percentages are not adopted as verified findings in this study.

## 4. Controlling your AI software factory

- [LeadDev on-demand panel](https://leaddev.com/technical-direction/controlling-your-ai-software-factory), page dated 24 October 2026.
- Moderator Jennifer Riggins; listed speakers Ciarán Flynn, Claire Reckless, and Johnny Austin.
- Evidence: **public abstract and speaker metadata only**. Video requires login. Source S33.

The public description covers oversight, intent, policy, and drift. It identifies useful questions to investigate, but the panel's answers were not accessible in this review. The page marks the material promoted partner content.

## 5. Lessons from Spec-Driven Development at CERN

- [CERN video record](https://videos.cern.ch/record/3025661).
- Evidence: **discovery metadata only**. The fetched page exposed site chrome rather than usable talk content. Source S34.

Retained as a reading-list lead, not evidence for architecture or productivity claims. Consult the recording and establish its speaker/date directly before citing specific lessons.

## 6. Official Codex learning library

- [OpenAI Codex learning collection](https://developers.openai.com/learn/codex).
- Evidence: **library listing only**, including entries for Shipping with Codex and code-review walkthroughs. Source S35.

Useful for execution and review demonstrations after choosing the integration surface. Current reference documentation remains the source for commands and API behavior; the individual videos were not analyzed.

## Date and discovery cautions

Search results surfaced [Moderne's AI Software Factory Summit](https://moderne.ai/ai-software-factory/). Its current page advertises **22–24 February 2027** and mixes future programming with past-event material. Upcoming session titles were excluded as evidence of completed talks. Similarly, November events occurring after this study's 8 November cutoff are future events, even when search results describe them prominently.

This collection contains one substantively reviewed talk transcript, one substantively reviewed slide deck, a chapter-based lesson review, and additional clearly marked leads. It does not claim a full audiovisual review of the media catalog.
