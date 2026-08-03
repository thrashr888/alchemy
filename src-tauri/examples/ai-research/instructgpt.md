Training Language Models to Follow Instructions (InstructGPT) - arXiv:2203.02155

Ouyang et al., 2022. The alignment recipe that turned a language model into an assistant.

A model trained to predict the next token on web text is good at continuing text, which is not the same as being helpful. Ask it a question and it may produce another question. The gap between the training objective and what users actually want is the problem this paper attacks.

The method is reinforcement learning from human feedback, in three stages. Collect demonstrations of desired behavior and supervise-fine-tune on them. Collect human rankings of model outputs and train a reward model to predict those preferences. Then optimize the language model against the reward model with PPO.

The headline result is how much it beats scale: outputs from the 1.3B InstructGPT were preferred by human labelers over those from the 175B GPT-3, a model over a hundred times larger. Truthfulness improved and toxic output declined, with only small regressions on standard NLP benchmarks. Essentially every deployed chat assistant descends from this recipe.

Read the paper: https://arxiv.org/abs/2203.02155
