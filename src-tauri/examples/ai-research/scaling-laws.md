Scaling Laws for Neural Language Models - arXiv:2001.08361

Kaplan et al., 2020. Turned "make it bigger" from a hunch into a forecast.

The paper measures how language model loss responds to three things: parameter count, dataset size, and compute. The finding is that the relationship is a smooth power law across more than seven orders of magnitude, with no sign of a floor in the range studied. Architectural details - depth versus width, for instance - matter remarkably little by comparison.

Two consequences shaped the years that followed. First, loss is predictable: you can train small models, fit the curve, and forecast what a much larger run will achieve before committing to it. Second, given a fixed compute budget, the authors argued the optimal strategy was to train very large models on relatively modest data and stop well before convergence.

That second conclusion was revised in 2022 by the Chinchilla work (arXiv:2203.15556), which found that models of the era were substantially undertrained and that parameters and tokens should scale roughly in proportion. Read the two together - the correction is as instructive as the original.

Read the paper: https://arxiv.org/abs/2001.08361
