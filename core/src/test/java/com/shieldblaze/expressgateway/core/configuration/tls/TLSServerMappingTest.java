package com.shieldblaze.expressgateway.core.configuration.tls;

import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.*;

class TLSServerMappingTest {

    @Test
    void defaultHost() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()),
                selfSignedCertificate.key(), false);

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

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()),
                selfSignedCertificate.key(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping();
        tlsServerMapping.addMapping("localhost", certificateKeyPair);
        tlsServerMapping.addMapping("*.localhost", certificateKeyPair);


        assertNull(tlsServerMapping.certificateKeyMap.get("DEFAULT_HOST"));
        assertEquals(certificateKeyPair, tlsServerMapping.certificateKeyMap.get("localhost"));
        assertEquals(certificateKeyPair, tlsServerMapping.certificateKeyMap.get("*.localhost"));

        assertThrows(IllegalArgumentException.class, () -> tlsServerMapping.addMapping("@.localhost", certificateKeyPair));
    }
}
