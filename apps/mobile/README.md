# Anchor Mobile

Captura sensores Android reais, converte frames nativos em `MotionSampleV1`, serializa JSON UTF-8 e envia um datagrama UDP por amostra para o desktop Anchor.

## Escopo atual

- Android apenas;
- destino IPv4 manual;
- porta padrao `57421`;
- um `MotionSampleV1` completo por datagrama;
- fila sequencial latest-wins com backpressure controlado;
- sem descoberta automatica;
- sem persistencia de IP/porta;
- sem ACK, autenticacao ou criptografia.

## Tela de diagnostico

A tela mostra:

- disponibilidade dos sensores obrigatorios;
- IP do computador e porta;
- estado do transporte UDP;
- destino atualmente usado;
- `sessionId`, `sequence`, idade da ultima amostra e taxa observada;
- datagramas oferecidos, enviados ao socket, descartes por backpressure, payloads rejeitados, erros de envio e ultimo erro.

O app nao afirma confirmacao de recebimento. `UDP nao confirma se o computador recebeu os dados` aparece explicitamente na tela.

## Como executar o fluxo completo

1. Descubra manualmente o IPv4 LAN do computador.
2. Inicie o desktop Anchor.
3. Confirme nos logs do desktop que o receptor escuta em `0.0.0.0:57421`.
4. Instale e abra o APK Android.
5. No celular, informe o IPv4 do computador e a porta `57421`.
6. Inicie o streaming.
7. No desktop, confirme:
   - sender correspondente ao celular;
   - mesma `sessionId` do mobile;
   - `sequence` crescente;
   - taxa proxima de `60 Hz`;
   - `received` e `accepted` crescendo.
8. Pare no celular.
9. Confirme no desktop a transicao para `stale` e depois `disconnected`.
10. Inicie novamente no celular e confirme nova `sessionId`.

## Observacoes importantes

- celular e computador devem estar na mesma rede Wi-Fi;
- a porta UDP `57421` precisa estar liberada no firewall local quando aplicavel;
- `127.0.0.1` no celular aponta para o proprio celular, nao para o computador;
- esta fase nao implementa ACK, autenticacao nem criptografia;
- nao exponha a porta UDP diretamente a internet.
