package com.shieldblaze.expressgateway.configuration.tls;

import java.util.List;

/**
 * <a href="https://wiki.mozilla.org/Security/Server_Side_TLS#Intermediate_compatibility_.28recommended.29">...</a>
 */
public final class IntermediateCrypto {

    public static final List<Protocol> PROTOCOLS = List.of(Protocol.TLS_1_3, Protocol.TLS_1_2);

    public static final List<Cipher> CIPHERS = List.of(
            Cipher.TLS_AES_256_GCM_SHA384,
            Cipher.TLS_AES_128_GCM_SHA256,
            Cipher.TLS_CHACHA20_POLY1305_SHA256,
            Cipher.TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            Cipher.TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            Cipher.TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
            Cipher.TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            Cipher.TLS_DHE_RSA_WITH_AES_256_GCM_SHA384,
            Cipher.TLS_DHE_RSA_WITH_AES_128_GCM_SHA256
    );

    private IntermediateCrypto() {
        // Prevent outside initialization
    }
}
