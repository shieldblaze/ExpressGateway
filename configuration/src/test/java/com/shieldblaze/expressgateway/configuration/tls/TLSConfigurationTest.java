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
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLException;
import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class TLSConfigurationTest {

    /**
     * Prevent mapping conflict in race condition
     */
    @BeforeEach
    void clearMappings() {
        TLSConfiguration.DEFAULT_SERVER.clearMappings();
    }

    @Test
    void cipherSetterTest() {
        assertThrows(IllegalArgumentException.class, () -> new TLSConfiguration().setCiphers(Collections.emptyList()));
        assertDoesNotThrow(() -> new TLSConfiguration().setCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384)));
    }

    @Test
    void protocolSetterTest() {
        assertThrows(IllegalArgumentException.class, () -> new TLSConfiguration().setProtocols(Collections.emptyList()));
        assertDoesNotThrow(() -> new TLSConfiguration().setProtocols(Collections.singletonList(Protocol.TLS_1_3)));
    }

    @Test
    void mutualTLSTest() {
        assertThrows(NullPointerException.class, () -> new TLSConfiguration().setMutualTLS(null));
        assertDoesNotThrow(() -> new TLSConfiguration().setMutualTLS(MutualTLS.REQUIRED));
    }

    @Test
    void sessionTimeoutTest() {
        assertThrows(IllegalArgumentException.class, () -> new TLSConfiguration().setSessionTimeout(-1));
        assertThrows(IllegalArgumentException.class, () -> new TLSConfiguration().setSessionTimeout(Integer.MIN_VALUE));
        assertDoesNotThrow(() -> new TLSConfiguration().setSessionTimeout(0));
        assertDoesNotThrow(() -> new TLSConfiguration().setSessionTimeout(1));
        assertDoesNotThrow(() -> new TLSConfiguration().setSessionTimeout(Integer.MAX_VALUE));
    }

    @Test
    void sessionCacheSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new TLSConfiguration().setSessionCacheSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new TLSConfiguration().setSessionCacheSize(Integer.MIN_VALUE));
        assertDoesNotThrow(() -> new TLSConfiguration().setSessionCacheSize(0));
        assertDoesNotThrow(() -> new TLSConfiguration().setSessionCacheSize(1));
        assertDoesNotThrow(() -> new TLSConfiguration().setSessionCacheSize(Integer.MAX_VALUE));
    }

    @Test
    void addMappingTest() throws CertificateException, SSLException {
        SelfSignedCertificate ssc = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(Collections.singletonList(ssc.cert()), ssc.key(), false);

        TLSConfiguration tlsConfiguration = TLSConfiguration.DEFAULT_SERVER;
        tlsConfiguration.addMapping("www.shieldblaze.com", certificateKeyPair);

        assertEquals(certificateKeyPair, tlsConfiguration.mapping("www.shieldblaze.com"));
        assertThrows(NullPointerException.class, () -> tlsConfiguration.mapping("shieldblaze.com"));
    }

    @Test
    void addMappingWildcardTest() throws CertificateException, SSLException {
        SelfSignedCertificate ssc = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(Collections.singletonList(ssc.cert()), ssc.key(), false);

        TLSConfiguration tlsConfiguration = TLSConfiguration.DEFAULT_SERVER;
        tlsConfiguration.addMapping("*.shieldblaze.com", certificateKeyPair);

        assertEquals(certificateKeyPair, tlsConfiguration.mapping("www.shieldblaze.com"));
        assertEquals(certificateKeyPair, tlsConfiguration.mapping("meow.shieldblaze.com"));
        assertThrows(NullPointerException.class, () -> tlsConfiguration.mapping("shieldblaze.com"));
    }

    @Test
    void removeMappingTest() throws CertificateException, SSLException {
        SelfSignedCertificate ssc = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(Collections.singletonList(ssc.cert()), ssc.key(), false);

        TLSConfiguration tlsConfiguration = TLSConfiguration.DEFAULT_SERVER;
        tlsConfiguration.addMapping("*.shieldblaze.com", certificateKeyPair);

        assertFalse(tlsConfiguration.removeMapping("www.shieldblaze.com"));
        assertFalse(tlsConfiguration.removeMapping("meow.shieldblaze.com"));
        assertFalse(tlsConfiguration.removeMapping("shieldblaze.com"));
        assertTrue(tlsConfiguration.removeMapping("*.shieldblaze.com"));
    }
}
