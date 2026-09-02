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

## Variantes Android

- `debug`: fluxo atual de desenvolvimento com Metro, Fast Refresh e suporte de developer preservados; pode usar `adb reverse tcp:8081 tcp:8081`.
- `standalone`: build interna nao-debuggable, com bundle JavaScript/Hermes incorporado, `applicationId` final `com.anchormobile.standalone`, `versionName` com sufixo `-standalone` e assinatura debug apenas para testes internos.
- `release`: reservado para uma futura distribuicao real; nao deve ser tratado como equivalente semantico da variante `standalone` e nao usa a debug keystore nesta fase.

## Pre-requisitos Android

- Node 20 ou superior;
- JDK 21 no `PATH` ou em `JAVA_HOME`;
- Android SDK acessivel por `ANDROID_HOME`, `ANDROID_SDK_ROOT` ou `apps/mobile/android/local.properties` nao versionado.

Se o ambiente nao estiver pronto, `pnpm build:mobile:standalone` falha cedo com a versao detectada do Java e a acao esperada para corrigir o SDK/Hermes/Gradle wrapper.

## Comandos repetiveis

```bash
pnpm dev:mobile
pnpm test:mobile:standalone-scripts
pnpm verify:mobile:bundle
pnpm build:mobile:standalone
```

- `pnpm test:mobile:standalone-scripts` executa os testes dos utilitarios de preflight e inspecao usados pela build standalone.
- `pnpm verify:mobile:bundle` gera um bundle Android real com `--dev false` usando `apps/mobile/index.js` e a `metro.config.js` do projeto, em diretoria temporaria limpa ao final.
- `pnpm build:mobile:standalone` valida o ambiente, executa `:app:assembleStandalone`, confirma `assets/index.android.bundle` dentro do APK e imprime caminho, tamanho, SHA-256, `applicationId` e a indicacao de assinatura interna.

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
- o APK `standalone` nao depende de Metro nem de `adb reverse` para abrir, mas continua enviando os datagramas por UDP/Wi-Fi ao IPv4 configurado manualmente;
- `debug` e `standalone` podem coexistir no mesmo Android por causa do sufixo `.standalone` no `applicationId`;
- a assinatura da variante `standalone` usa a debug keystore apenas para testes internos e e inadequada para distribuicao;
- esta fase nao implementa ACK, autenticacao nem criptografia;
- nao exponha a porta UDP diretamente a internet.
