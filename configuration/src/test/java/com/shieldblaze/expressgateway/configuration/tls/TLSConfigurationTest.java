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

import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerMapping;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class TLSConfigurationTest {

    @Test
    void test() throws CertificateException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()),
                selfSignedCertificate.key(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);
        tlsServerMapping.addMapping("*.localhost", certificateKeyPair);

        TLSConfiguration tlsConfiguration = new TLSConfiguration();
        tlsConfiguration.setCertificateKeyPairMap(tlsServerMapping.certificateKeyMap);

        assertEquals(certificateKeyPair, tlsConfiguration.getMapping("haha.localhost"));
        assertEquals(certificateKeyPair, tlsConfiguration.getMapping("123.localhost"));
        assertEquals(certificateKeyPair, tlsConfiguration.getMapping("localhost.localhost"));
        assertEquals(certificateKeyPair, tlsConfiguration.getMapping("shieldblaze.com"));
    }
}