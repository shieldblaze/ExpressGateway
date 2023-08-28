/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLException;
import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class TlsConfigurationTest {

    /**
     * Prevent mapping conflict in race condition
     */
    @BeforeEach
    void clearMappings() {
        TlsServerConfiguration.DEFAULT.clearMappings();
    }

    @Test
    void cipherSetterTest() {
        assertThrows(IllegalArgumentException.class, () -> new TlsServerConfiguration().setCiphers(Collections.emptyList()));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384)));
    }

    @Test
    void protocolSetterTest() {
        assertThrows(IllegalArgumentException.class, () -> new TlsServerConfiguration().setProtocols(Collections.emptyList()));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setProtocols(Collections.singletonList(Protocol.TLS_1_3)));
    }

    @Test
    void mutualTLSTest() {
        assertThrows(NullPointerException.class, () -> new TlsServerConfiguration().setMutualTLS(null));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setMutualTLS(MutualTLS.REQUIRED));
    }

    @Test
    void sessionTimeoutTest() {
        assertThrows(IllegalArgumentException.class, () -> new TlsServerConfiguration().setSessionTimeout(-1));
        assertThrows(IllegalArgumentException.class, () -> new TlsServerConfiguration().setSessionTimeout(Integer.MIN_VALUE));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setSessionTimeout(0));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setSessionTimeout(1));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setSessionTimeout(Integer.MAX_VALUE));
    }

    @Test
    void sessionCacheSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new TlsServerConfiguration().setSessionCacheSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new TlsServerConfiguration().setSessionCacheSize(Integer.MIN_VALUE));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setSessionCacheSize(0));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setSessionCacheSize(1));
        assertDoesNotThrow(() -> new TlsServerConfiguration().setSessionCacheSize(Integer.MAX_VALUE));
    }

    @Test
    void addMappingTest() throws Exception {
        SelfSignedCertificate ssc = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(Collections.singletonList(ssc.cert()), ssc.key(), false);

        TlsConfiguration tlsConfiguration = TlsServerConfiguration.DEFAULT;
        tlsConfiguration.addMapping("www.shieldblaze.com", certificateKeyPair);

        assertEquals(certificateKeyPair, tlsConfiguration.mapping("www.shieldblaze.com"));
        assertNull(tlsConfiguration.mapping("shieldblaze.com"));
    }

    @Test
    void addMappingWildcardTest() throws Exception {
        SelfSignedCertificate ssc = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(Collections.singletonList(ssc.cert()), ssc.key(), false);

        TlsConfiguration tlsConfiguration = TlsServerConfiguration.DEFAULT;
        tlsConfiguration.addMapping("*.shieldblaze.com", certificateKeyPair);

        assertEquals(certificateKeyPair, tlsConfiguration.mapping("www.shieldblaze.com"));
        assertEquals(certificateKeyPair, tlsConfiguration.mapping("meow.shieldblaze.com"));
        assertNull(tlsConfiguration.mapping("shieldblaze.com"));
    }

    @Test
    void removeMappingTest() throws Exception {
        SelfSignedCertificate ssc = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(Collections.singletonList(ssc.cert()), ssc.key(), false);

        TlsConfiguration tlsConfiguration = TlsServerConfiguration.DEFAULT;
        tlsConfiguration.addMapping("*.shieldblaze.com", certificateKeyPair);

        assertFalse(tlsConfiguration.removeMapping("www.shieldblaze.com"));
        assertFalse(tlsConfiguration.removeMapping("meow.shieldblaze.com"));
        assertFalse(tlsConfiguration.removeMapping("shieldblaze.com"));
        assertTrue(tlsConfiguration.removeMapping("*.shieldblaze.com"));
    }
}
