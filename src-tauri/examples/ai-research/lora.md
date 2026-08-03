LoRA: Low-Rank Adaptation of Large Language Models - arXiv:2106.09685

Hu et al., 2021. Made fine-tuning large models affordable, which made an ecosystem of them possible.

Full fine-tuning updates every parameter and produces a complete copy of the model per task - untenable when the model has billions of parameters and you want dozens of variants. LoRA starts from the observation that the weight *update* during adaptation tends to have low intrinsic rank, even when the weights themselves do not.

So instead of learning a full-rank update to a weight matrix, LoRA freezes the pre-trained weights and learns two small matrices whose product forms the update. Rank is a hyperparameter, typically small. Trainable parameters drop by orders of magnitude, optimizer memory drops with them, and because the learned product can be folded into the original weights after training, inference has no added latency - unlike adapter layers.

Multiple LoRAs can be trained against one frozen base and swapped per task. That property, more than the efficiency, is what made the technique ubiquitous: it turned a large model into a platform other people could specialize cheaply.

Read the paper: https://arxiv.org/abs/2106.09685
