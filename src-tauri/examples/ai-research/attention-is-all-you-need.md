Attention Is All You Need - arXiv:1706.03762

Vaswani et al., 2017. The paper that introduced the Transformer, and with it essentially every large language model that followed.

Before it, sequence transduction meant recurrence: an RNN or LSTM walked the input one position at a time, carrying a hidden state. That serialization capped how much could be parallelized during training, and long-range dependencies had to survive many steps of state to matter. Convolutional approaches parallelized better but needed depth proportional to distance to relate two positions.

The Transformer dispenses with both. It relates any two positions in a single operation using scaled dot-product attention, run in parallel across multiple heads so different heads can attend to different kinds of relationships. Position information, no longer implicit in the order of computation, is added explicitly through positional encodings. The whole encoder-decoder stack is attention plus feed-forward layers plus residual connections and layer normalization.

The results were state of the art on WMT 2014 English-to-German (28.4 BLEU) and English-to-French (41.8 BLEU) at a small fraction of the training cost of the models it beat. The lasting contribution was not the translation numbers, though, but the architecture's scaling behavior: attention layers parallelize across sequence positions, which is exactly what makes it economical to train very large models on very large corpora.

Read the paper: https://arxiv.org/abs/1706.03762
