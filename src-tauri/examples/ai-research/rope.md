RoFormer: Rotary Position Embedding (RoPE) - arXiv:2104.09864

Su et al., 2021. The position encoding that modern LLMs actually use.

Transformers need position information injected, and the original design's additive sinusoidal encodings were one of its least settled choices. Learned absolute positions don't extrapolate past training length; earlier relative-position schemes bolted extra terms onto attention.

RoPE encodes position by rotation. Each query and key vector is split into two-dimensional pairs, and each pair is rotated by an angle proportional to the token's position, at a spectrum of frequencies. The elegance is what happens in the dot product: the rotations compose so that attention between two tokens depends only on their relative offset. Absolute encoding in, relative behavior out - with no change to the attention computation itself and a natural decay of interaction with distance.

Published quietly by a small Chinese team, it spread through adoption rather than citation: GPT-NeoX, PaLM, LLaMA, and nearly every open model since carry it. It also became the lever for context extension - the frequency base can be rescaled to stretch a trained model's window, which is how most "long context" variants of open models are made.

Read the paper: https://arxiv.org/abs/2104.09864
