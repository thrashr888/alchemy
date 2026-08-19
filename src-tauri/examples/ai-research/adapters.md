Adapters: Parameter-Efficient Transfer Learning for NLP - arXiv:1902.00751

Houlsby et al., 2019. Named the problem an ecosystem now lives inside: adapt a big frozen model by training almost nothing.

BERT had just made fine-tuning the default recipe, and the cost was visible immediately: a full copy of the model per task. For anyone serving many tasks, parameters multiplied and catastrophic forgetting loomed. This paper asks how little you can train and still match full fine-tuning.

Its answer is the adapter: a small bottleneck network - project down, nonlinearity, project up, residual connection - inserted after each sublayer of a frozen Transformer. Initialized near-identity so training starts from the pre-trained model's behavior, adapters reach within 0.4% of full fine-tuning on GLUE while training about 3% of the parameters per task.

The specific module mattered less than the framing. Parameter-efficient fine-tuning became a field with a name - prefix tuning, prompt tuning, BitFit, and LoRA all answer this paper's question with different trainable slivers - and the serve-one-base-swap-small-modules deployment pattern it introduced is now how model customization works everywhere.

Read the paper: https://arxiv.org/abs/1902.00751
