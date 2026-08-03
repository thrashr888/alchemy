Language Models are Few-Shot Learners (GPT-3) - arXiv:2005.14165

Brown et al., 2020. The scaling result that made prompting a programming interface.

GPT-3 is architecturally unremarkable - a 175-billion-parameter autoregressive Transformer, roughly ten times larger than anything before it. What made the paper important was what emerged at that scale: the model could perform tasks it had never been fine-tuned on, given only a natural-language description and a handful of examples in its prompt.

The authors call this in-context learning, and they evaluate it in three regimes - zero-shot (instruction only), one-shot, and few-shot (a handful of demonstrations). Performance improves smoothly with model size across all three, and the gap between few-shot and fine-tuned models narrows as scale increases. On some tasks GPT-3 approached or matched fine-tuned state-of-the-art systems without a single gradient update.

The paper is unusually candid about limits: the model struggles with tasks requiring genuine bidirectional reasoning, it can be incoherent over long passages, and the authors devote substantial space to bias, misuse, and energy cost. Its practical consequence was to shift the field's interface from training to prompting, and to make scale the first thing anyone tried.

Read the paper: https://arxiv.org/abs/2005.14165
