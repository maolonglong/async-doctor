# m2_venv_cold_warm_realistic

Multi-package venv (dotenv, agents-sdk, libx→liby) exercising third-party
internal sinks and cross-package follow. Cold, warm, and a second warm must
agree byte-for-byte (no extra site-packages candidates on warm).
