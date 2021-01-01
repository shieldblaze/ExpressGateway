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

import java.security.cert.CertificateException;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class TLSConfigurationTest {

    @Test
    void test() throws CertificateException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(selfSignedCertificate.certificate().getAbsolutePath(),
                selfSignedCertificate.privateKey().getAbsolutePath(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);
        tlsServerMapping.mapping("*.localhost", certificateKeyPair);

        TLSConfiguration tlsConfiguration = new TLSConfiguration();
        tlsConfiguration.certificateKeyPairMap(tlsServerMapping.certificateKeyMap);

        assertEquals(certificateKeyPair, tlsConfiguration.mapping("haha.localhost"));
        assertEquals(certificateKeyPair, tlsConfiguration.mapping("123.localhost"));
        assertEquals(certificateKeyPair, tlsConfiguration.mapping("localhost.localhost"));
        assertEquals(certificateKeyPair, tlsConfiguration.mapping("shieldblaze.com"));
    }
}
