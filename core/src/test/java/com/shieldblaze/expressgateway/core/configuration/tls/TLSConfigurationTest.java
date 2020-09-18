package com.shieldblaze.expressgateway.core.configuration.tls;

import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.*;

class TLSConfigurationTest {

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
