Deep Residual Learning for Image Recognition (ResNet) - arXiv:1512.03385

He et al., 2015. Made very deep networks trainable, and won ImageNet 2015 doing it.

Stacking more layers should not make a network worse - the extra layers could always learn the identity function. Empirically, though, deeper plain networks had higher training error than shallower ones, which ruled out overfitting and pointed at an optimization problem: the identity mapping turns out to be hard for a stack of nonlinear layers to learn.

The residual block sidesteps this. Instead of asking a group of layers to learn a target mapping H(x), it asks them to learn the residual F(x) = H(x) - x, and adds x back through a shortcut connection. If the identity is optimal, the layers only need to drive F toward zero, which is easy. The shortcuts add no parameters and no meaningful computation.

With them, the authors trained networks of 152 layers - eight times deeper than VGG - with lower complexity, and took first place in ILSVRC 2015 classification, detection, and localization, plus COCO detection and segmentation. Residual connections are now standard structure in essentially every deep architecture, Transformers very much included.

Read the paper: https://arxiv.org/abs/1512.03385
