Chain-of-Thought Prompting Elicits Reasoning - arXiv:2201.11903

Wei et al., 2022. A prompting change that unlocked multi-step reasoning in large models.

Large language models were strikingly bad at arithmetic word problems and other multi-step tasks, even at scales where they excelled at everything else. The intervention here is almost trivially simple: in the few-shot examples, do not just show the answer - show the intermediate reasoning steps that lead to it. The model then produces its own reasoning before answering.

The gains are large and, importantly, emergent with scale. Below roughly 100B parameters chain-of-thought prompting helps little or hurts; above it, the improvements are dramatic. On GSM8K math word problems, prompting a 540B model this way beat a fine-tuned GPT-3 with a verifier.

The paper is worth reading alongside its skeptics: whether the generated chain reflects the computation actually producing the answer is genuinely unresolved, and a plausible-looking chain can accompany a wrong answer. Still, this is the origin of nearly all subsequent reasoning work, including the trained-in reasoning of current frontier models.

Read the paper: https://arxiv.org/abs/2201.11903
