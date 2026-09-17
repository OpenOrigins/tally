from unittest.mock import Mock

from tally_langgraph import _tls


def test_https_handler_uses_certifi_bundle(monkeypatch) -> None:
    context = Mock()
    create_context = Mock(return_value=context)
    monkeypatch.setattr(_tls.certifi, "where", lambda: "/trusted/cacert.pem")
    monkeypatch.setattr(_tls.ssl, "create_default_context", create_context)

    handler = _tls.https_handler()

    create_context.assert_called_once_with(cafile="/trusted/cacert.pem")
    assert handler._context is context
