from . import db, graph


def main():
    db.init_db()
    print("GitHub Monitor Agent -- ask about repo activity, e.g. 'what happened today?' "
          "or 'give me a daily report'. Ctrl+C to exit.\n")
    while True:
        try:
            user_input = input("you> ").strip()
        except (EOFError, KeyboardInterrupt):
            print()
            break
        if not user_input:
            continue
        # Every question activates the same pipeline as webhook events do; its log_input
        # node writes this question to the .db before gather_facts/summarize_report run.
        result = graph.run("chat", text=user_input)
        print(f"\nagent> {result.get('final_response', '')}\n")


if __name__ == "__main__":
    main()
