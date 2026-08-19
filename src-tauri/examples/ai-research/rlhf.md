Deep Reinforcement Learning from Human Preferences (RLHF) - arXiv:1706.03741

Christiano et al., 2017. The alignment recipe five years before it aligned anything large: learn what people want by asking them to compare.

Reinforcement learning needs a reward function, and for most things people actually care about, nobody can write one down. Hand-specified rewards get gamed; demonstrations require the demonstrator to be able to do the task. This paper's move is to learn the reward instead, from the cheapest signal available: show a human two short clips of agent behavior and ask which is better.

A reward model is trained on those comparisons, an RL agent optimizes the learned reward, and the loop continues with fresh comparisons where the reward model is most uncertain. On Atari and simulated robotics, about an hour of human comparisons taught behaviors - a backflip, most famously - that nobody could have specified as a reward formula.

The paper also documents reward hacking, the failure mode the field still fights. Swap the clips for pairs of model responses and this pipeline is, almost line for line, the RLHF that InstructGPT applied to language models.

Read the paper: https://arxiv.org/abs/1706.03741
