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

import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

final class TLSServerMappingTest {

    @Test
    void defaultHost() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(selfSignedCertificate.certificate().getAbsolutePath(),
                selfSignedCertificate.privateKey().getAbsolutePath(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);
        assertEquals(certificateKeyPair, tlsServerMapping.certificateKeyMap.get("DEFAULT_HOST"));
    }

    @Test
    void defaultNotFound() {
        TLSServerMapping tlsServerMapping = new TLSServerMapping();
        assertNull(tlsServerMapping.certificateKeyMap.get("DEFAULT_HOST"));
    }

    @Test
    void addMapping() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(selfSignedCertificate.certificate().getAbsolutePath(),
                selfSignedCertificate.privateKey().getAbsolutePath(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping();
        tlsServerMapping.mapping("localhost", certificateKeyPair);
        tlsServerMapping.mapping("*.localhost", certificateKeyPair);


        assertNull(tlsServerMapping.certificateKeyMap.get("DEFAULT_HOST"));
        assertEquals(certificateKeyPair, tlsServerMapping.certificateKeyMap.get("localhost"));
        assertEquals(certificateKeyPair, tlsServerMapping.certificateKeyMap.get("*.localhost"));

        assertThrows(IllegalArgumentException.class, () -> tlsServerMapping.mapping("@.localhost", certificateKeyPair));
    }
}
