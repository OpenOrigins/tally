from . import db, graph


def main() -> None:
    db.init_db()
    print(
        "GitHub Monitor Agent -- ask about repo activity, e.g. 'what happened today?' "
        "or 'give me a daily report'. Ctrl+C to exit.\n"
    )
    try:
        while True:
            try:
                user_input = input("you> ").strip()
            except (EOFError, KeyboardInterrupt):
                print()
                break
            if not user_input:
                continue
            result = graph.run("chat", text=user_input)
            print(f"\nagent> {result.get('final_response', '')}\n")
    finally:
        graph.close()


if __name__ == "__main__":
    main()
