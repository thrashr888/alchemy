BERT: Pre-training of Deep Bidirectional Transformers - arXiv:1810.04805

Devlin et al., 2018. Established the pre-train-then-fine-tune recipe that dominated NLP before generative models took over.

Earlier pre-trained language models read left to right, because that is what language modeling requires: predicting the next token from the previous ones. BERT's argument was that for understanding tasks - classification, question answering, entailment - conditioning on both directions at once matters more, and the unidirectional constraint is an artifact of the training objective rather than a requirement of the task.

Its solution is masked language modeling: hide a fraction of the input tokens and train the model to recover them from the full surrounding context, left and right. A second objective, next-sentence prediction, was meant to teach relationships between sentence pairs, though later work found it largely unnecessary.

BERT advanced the state of the art across eleven NLP tasks, and its practical legacy was the workflow: pre-train one large model on unlabeled text, then fine-tune a small head per task. That pattern made strong NLP available to teams without the resources to train from scratch, and encoder-style BERT descendants remain the workhorses of embedding and retrieval - including the models that power semantic search in tools like this one.

Read the paper: https://arxiv.org/abs/1810.04805
