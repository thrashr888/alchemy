Multi-Query Attention: One Write-Head is All You Need - arXiv:1911.02150

Shazeer, 2019. The paper behind the KV cache economics of every deployed LLM.

Autoregressive generation is incremental: each new token attends over all previous ones, so implementations cache the keys and values of past positions rather than recompute them. This "KV cache" was folklore until this paper, which is worth reading for its clear accounting alone - at generation time the bottleneck is not arithmetic but memory bandwidth, and the cache is what saturates it. Every attention head keeping its own keys and values means the cache scales with heads times sequence length, and loading it dominates the cost of producing each token.

The proposal, multi-query attention, keeps many query heads but shares a single key head and value head across all of them. The cache shrinks by the head count, decoding speeds up several-fold, and quality drops only slightly.

Sharing keys and values among groups of query heads rather than all of them - grouped-query attention - recovers most of the lost quality, and MQA or GQA now sits in Llama, Mistral, Gemini, and essentially every model that serves traffic.

Read the paper: https://arxiv.org/abs/1911.02150
