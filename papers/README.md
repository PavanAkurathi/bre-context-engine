# BRE Papers

This directory contains the formal BRE math paper and its bibliography.

## Files

- `formal-bounded-context-selection.tex` - formal bounded-context selection paper.
- `formal-bounded-context-selection.bib` - bibliography for the formal paper.

The practical/product-oriented BRE overview remains at the repository root in `whitepaper.tex`.

## Install LaTeX on macOS

Full TeX install:

```sh
brew install --cask mactex
```

Smaller install:

```sh
brew install --cask basictex
sudo tlmgr update --self
sudo tlmgr install latexmk collection-latexrecommended collection-fontsrecommended
```

After either install, restart the terminal or make TeX visible in the current shell:

```sh
export PATH="/Library/TeX/texbin:$PATH"
```

## Build the Formal Paper

Using `latexmk`:

```sh
cd /Users/av/Documents/bre/papers
latexmk -pdf formal-bounded-context-selection.tex
```

Manual build:

```sh
cd /Users/av/Documents/bre/papers
pdflatex formal-bounded-context-selection.tex
bibtex formal-bounded-context-selection
pdflatex formal-bounded-context-selection.tex
pdflatex formal-bounded-context-selection.tex
```

The generated PDF will be `formal-bounded-context-selection.pdf`.
