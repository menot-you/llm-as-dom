# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
## [0.13.0](https://github.com/menot-you/llm-as-dom/compare/v0.12.0...v0.13.0) - 2026-04-17

### Bug Fixes
- *(backend)* Plumb --llm-url to openai/anthropic constructors
- *(chromium)* Add --single-process + --disable-dev-shm-usage
- *(chromium)* Pass --no-zygote when sandbox is disabled
- *(python)* Copy README.md to each python package to fix OutsideDestinationError
- *(deny)* Allow Unicode-3.0, MPL-2.0, CDLA-Permissive-2.0 licenses
- *(npm)* Restore org scope and remove manual action
- *(npm)* Remove org scope to match unscoped published package name
- *(ci)* Dead code gate, audit build tooling, pin ecosystems publish


### Features
- Add LLM-as-DOM demonstration scripts and Python wrapper for MCP server binary distribution

