ReAct: Synergizing Reasoning and Acting in Language Models - arXiv:2210.03629

Yao et al., 2022. The loop that turned language models into agents.

Chain-of-thought showed models could reason before answering, but the reasoning ran on the model's own possibly-stale knowledge, with nothing to check it. Tool-use work had models emitting actions, but without deliberation about which action or what the results meant. ReAct interleaves the two: the model alternates between thought traces ("I need the elevation, let me search for it"), actions against an environment (a search query, a lookup, a command), and observations of what came back, until it can finish.

The interleaving is the contribution. Reasoning decides the next action; observations ground the next reasoning step. On knowledge tasks the loop cuts hallucination by letting claims be checked mid-stream; on interactive environments (ALFWorld, WebShop) it beats both pure reasoning and pure acting by wide margins.

Read it as the ancestor of every "agent" shipping today - the thought/action/observation cycle is, almost unchanged, the inner loop of coding assistants and computer-use systems. Its failure modes survived too: loops, and reasoning that rationalizes a bad observation.

Read the paper: https://arxiv.org/abs/2210.03629
