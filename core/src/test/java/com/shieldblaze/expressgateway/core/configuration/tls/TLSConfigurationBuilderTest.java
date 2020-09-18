package com.shieldblaze.expressgateway.core.configuration.tls;

import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.*;

class TLSConfigurationBuilderTest {

    @Test
    void test() throws CertificateException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);
        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()),
                selfSignedCertificate.key(), false);
        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);
        tlsServerMapping.addMapping("*.localhost", certificateKeyPair);

        assertThrows(NullPointerException.class, () -> TLSConfigurationBuilder.forServer().withCiphers(null).build());

        assertThrows(NullPointerException.class, () -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_128_GCM_SHA256))
                .withProtocols(null)
                .build());

        assertThrows(NullPointerException.class, () -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .build());

        assertThrows(NullPointerException.class, () -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withMutualTLS(null)
                .build());

        assertThrows(NullPointerException.class, () -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTLSServerMapping(null)
                .build());

        assertThrows(IllegalArgumentException.class, () -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTLSServerMapping(tlsServerMapping)
                .withUseALPN(true)
                .withSessionTimeout(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTLSServerMapping(tlsServerMapping)
                .withUseALPN(true)
                .withSessionTimeout(10)
                .withSessionCacheSize(-1)
                .build());

        assertDoesNotThrow(() -> TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTLSServerMapping(tlsServerMapping)
                .withUseALPN(true)
                .withSessionTimeout(10)
                .withSessionCacheSize(10)
                .build());
    }
}
