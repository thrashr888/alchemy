CLIP: Learning Transferable Visual Models From Natural Language Supervision - arXiv:2103.00020

Radford et al., 2021. Trained vision and language into one embedding space, and made image classifiers you never fine-tune.

Computer vision had been supervised by fixed label sets: a model trained on ImageNet's thousand classes knows those thousand classes and nothing else. CLIP replaces the labels with natural language. Train an image encoder and a text encoder jointly on 400 million image-caption pairs, with a contrastive objective: each image's embedding should be close to its own caption's embedding and far from the other captions in the batch.

What falls out is zero-shot classification. To recognize a category no one trained for, embed the phrase "a photo of a dog" and ask which candidate text sits closest to the image. On ImageNet, this matches the accuracy of the original supervised ResNet-50 without seeing a single ImageNet training example - and it holds up far better under distribution shift.

CLIP's encoders became infrastructure: they steer text-to-image diffusion models, seed vision-language models, and power most image search. The paper is also candid about limits - counting, fine-grained tasks, and anything far from web imagery remain weak.

Read the paper: https://arxiv.org/abs/2103.00020
