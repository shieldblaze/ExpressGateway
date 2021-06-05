package com.shieldblaze.expressgateway.configuration.tls;

import java.util.List;

/**
 * https://wiki.mozilla.org/Security/Server_Side_TLS#Modern_compatibility
 */
public final class ModernCrypto {

    public static final List<Protocol> PROTOCOLS = List.of(Protocol.TLS_1_3);

    public static final List<Cipher> CIPHERS = List.of(
            Cipher.TLS_AES_256_GCM_SHA384,
            Cipher.TLS_AES_128_GCM_SHA256,
            Cipher.TLS_CHACHA20_POLY1305_SHA256
    );
}
