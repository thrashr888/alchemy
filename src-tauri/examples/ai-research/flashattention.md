FlashAttention: Fast and Memory-Efficient Exact Attention - arXiv:2205.14135

Dao et al., 2022. Made exact attention IO-bound math instead of memory-bound folklore, and long contexts affordable.

Attention's quadratic cost had spawned a cottage industry of approximations - sparse patterns, low-rank projections, kernel tricks - most trading quality for speed. This paper's diagnosis is that the real bottleneck was never the arithmetic but the memory traffic: standard implementations materialize the full attention matrix in GPU high-bandwidth memory and read it back repeatedly.

FlashAttention computes the same exact result without ever materializing that matrix. It tiles the computation to fit in on-chip SRAM, streams over blocks of keys and values with an online softmax that maintains running normalization statistics, and recomputes the attention matrix during the backward pass rather than storing it. More arithmetic, far less memory movement - and on hardware where compute outruns bandwidth, that trade wins: several-fold wall-clock speedups and memory linear in sequence length.

It ended the approximate-attention era almost single-handedly; exact attention at 32K and beyond became routine, and its successors ship inside the standard attention kernels of every major framework.

Read the paper: https://arxiv.org/abs/2205.14135
