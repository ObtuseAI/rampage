import os


def main() -> None:
    # Instrumentation plugins are optional and can rely on source files that are
    # absent in a one-file build. Rampage has its own evidence ledger and tracing.
    os.environ.setdefault("PYDANTIC_DISABLE_PLUGINS", "__all__")
    from rampage_intelligence.api import main as api_main

    api_main()


if __name__ == "__main__":
    main()
