Denoising Diffusion Probabilistic Models - arXiv:2006.11239

Ho et al., 2020. Turned diffusion models into a practical rival to GANs, and set up the image-generation era that followed.

A diffusion model defines a fixed forward process that gradually adds Gaussian noise to data over many steps until nothing but noise remains, then learns to reverse it. Generation means starting from noise and running the learned reverse process back to a sample. The idea predates this paper; what this paper contributed was making it work well.

The central simplification is in the training objective. Rather than optimizing the variational bound directly, the authors reparameterize the model to predict the noise added at each step, which reduces training to a weighted denoising regression - simple, stable, and well-behaved. They also connect the resulting objective to denoising score matching and Langevin dynamics.

The samples were competitive with the best GANs of the time, without adversarial training's instability. The architecture and objective here are recognizably the basis of the text-to-image systems that followed.

Read the paper: https://arxiv.org/abs/2006.11239
