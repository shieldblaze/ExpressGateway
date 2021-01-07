/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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

import javax.net.ssl.SSLException;
import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class TLSConfigurationTest {

    @Test
    void test() throws CertificateException, SSLException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(selfSignedCertificate.certificate().getAbsolutePath(),
                selfSignedCertificate.privateKey().getAbsolutePath(), false);

        TLSServerConfiguration tlsServerConfiguration = new TLSServerConfiguration();
        tlsServerConfiguration.ciphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384));
        tlsServerConfiguration.protocols(Collections.singletonList(Protocol.TLS_1_3));
        tlsServerConfiguration.mutualTLS(MutualTLS.NOT_REQUIRED);

        tlsServerConfiguration.addMapping("*.localhost", certificateKeyPair);

        assertEquals(certificateKeyPair, tlsServerConfiguration.getMapping("haha.localhost"));
        assertEquals(certificateKeyPair, tlsServerConfiguration.getMapping("123.localhost"));
        assertEquals(certificateKeyPair, tlsServerConfiguration.getMapping("localhost.localhost"));
        assertEquals(certificateKeyPair, tlsServerConfiguration.getMapping("shieldblaze.localhost"));
    }
}
