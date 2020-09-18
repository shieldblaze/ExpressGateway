package com.shieldblaze.expressgateway.core.configuration.tls;

import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.*;

class CertificateKeyPairTest {

    @Test
    void test() throws CertificateException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);
        assertThrows(NullPointerException.class, () -> new CertificateKeyPair(null, null, false));
        assertThrows(IllegalArgumentException.class, () -> new CertificateKeyPair(Collections.emptyList(), null, false));
        assertThrows(NullPointerException.class, () -> new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()),
                null, false));
        assertDoesNotThrow(() -> new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key(),
                false));
    }
}
