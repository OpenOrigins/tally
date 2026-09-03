import ollama

response = ollama.chat(
    model="llama3.2",
    messages=[
        {"role": "user", "content": "Hello! What is your role?"}
    ]
)

print(response.message.content)
