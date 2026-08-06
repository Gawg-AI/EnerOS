.DEFAULT_GOAL := blocked

.PHONY: blocked all build run flash legacy-research

blocked all build run flash legacy-research:
	@echo "Legacy Makefile product/research entry is retired." >&2
	@echo "Use the single formal entry: cargo xtask <audit|build|image|run|inspect|flash>." >&2
	@false
