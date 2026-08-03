Lost in the Middle: How Language Models Use Long Contexts - arXiv:2307.03172

Liu et al., 2023. A pointed empirical result about long context windows, and required reading for anyone building retrieval systems.

Context windows had been growing quickly, and the implicit assumption was that a longer window means more usable information. This paper tests that directly, using multi-document question answering and key-value retrieval tasks where the position of the relevant document can be varied while everything else is held constant.

Performance turns out to depend strongly on where the relevant information sits. Models do best when it appears at the very beginning or the very end of the context, and measurably worse when it is buried in the middle - a U-shaped curve reminiscent of primacy and recency effects in human memory. The degradation is substantial, and it holds for explicitly long-context models too: simply extending the window does not fix it.

The practical implication for retrieval-augmented systems is direct. Retrieving more passages is not automatically better - beyond a point extra context hurts - and the ordering of retrieved passages is a real design decision, not an implementation detail.

Read the paper: https://arxiv.org/abs/2307.03172
