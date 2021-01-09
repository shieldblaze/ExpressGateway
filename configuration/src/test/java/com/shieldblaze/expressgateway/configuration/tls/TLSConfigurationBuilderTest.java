/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.configuration.tls;

import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

final class TLSConfigurationBuilderTest {

    @Test
    void test() throws CertificateException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);
        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()),
                selfSignedCertificate.key(), false);
        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);
        tlsServerMapping.mapping("*.localhost", certificateKeyPair);

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
