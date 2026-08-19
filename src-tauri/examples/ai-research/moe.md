Sparsely-Gated Mixture-of-Experts - arXiv:1701.06538

Shazeer et al., 2017. Decoupled a model's parameter count from its compute per token - the trick behind most of today's largest models.

The observation is that a dense network spends every parameter on every input. A mixture-of-experts layer instead holds many parallel feed-forward "experts" and a small gating network that routes each token to a few of them. Parameters scale with the number of experts; compute scales only with how many each token visits. This paper made the idea work at scale - over a thousand experts, 137 billion parameters in 2017 - by confronting the unglamorous parts: a noisy top-k gate, an auxiliary loss to keep expert load balanced, and batching tricks so sparse routing stays hardware-efficient.

Applied between LSTM layers on language modeling and translation, it beat dense state-of-the-art at a fraction of the compute.

The recurrent host aged out; the layer did not. Transplanted into Transformers (Switch, GLaM, Mixtral, and most frontier models since), sparse expert layers are now the standard way to grow capacity without growing per-token cost - and the load-balancing loss here is still in the recipes.

Read the paper: https://arxiv.org/abs/1701.06538
