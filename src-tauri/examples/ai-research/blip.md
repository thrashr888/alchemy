BLIP: Bootstrapping Language-Image Pre-training - arXiv:2201.12086

Li et al., 2022. Unified vision-language understanding and generation, and cleaned its own training data with itself.

CLIP-style contrastive models are strong at matching images to text but cannot generate a caption; caption-generating models tend to be weaker at retrieval. BLIP's architecture serves both: one image encoder, and a text side that can act as an encoder (for alignment), a grounded encoder (for image-text matching), or a decoder (for captioning), with most weights shared across the three modes.

The second contribution is the bootstrap. Web-scraped alt-text is noisy, so BLIP trains on it once, then uses the resulting captioner to write synthetic captions and the resulting matcher to filter both the synthetic and the original ones. Training on the cleaned mixture beats training on more raw data - an early, concrete demonstration that model-filtered data can outrun data volume.

BLIP set marks on retrieval, captioning, and VQA at modest scale, and its successor's Q-Former became a standard bridge between frozen image encoders and frozen language models. Most current multimodal chat assistants inherit from this line.

Read the paper: https://arxiv.org/abs/2201.12086
