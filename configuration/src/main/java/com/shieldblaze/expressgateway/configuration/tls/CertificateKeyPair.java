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

import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import io.netty.handler.ssl.SslContext;
import org.bouncycastle.cert.ocsp.BasicOCSPResp;
import org.bouncycastle.cert.ocsp.OCSPResp;
import org.bouncycastle.cert.ocsp.SingleResp;

import java.io.Closeable;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@link X509Certificate} and {@link PrivateKey} Pair
 */
public final class CertificateKeyPair implements Runnable, Closeable {

    @Expose
    private final String certificateChain;

    private final List<X509Certificate> certificates = new ArrayList<>();

    @Expose
    private final String privateKey;

    @Expose
    private boolean useOCSPStapling;

    private volatile byte[] ocspStaplingData = null;
    private ScheduledFuture<?> scheduledFuture;
    private SslContext sslContext;

    public CertificateKeyPair(String certificateChain, String privateKey, boolean useOCSPStapling) {
        this.certificateChain = Objects.requireNonNull(certificateChain, "Certificate Chain");
        this.privateKey = Objects.requireNonNull(privateKey, "Private Key");

        if (useOCSPStapling) {
            scheduledFuture = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(this, 0, 6, TimeUnit.HOURS);
        }
        this.useOCSPStapling = useOCSPStapling;

        try {
            CertificateFactory cf = CertificateFactory.getInstance("X.509");
            for (Certificate certificate : cf.generateCertificates(new FileInputStream(certificateChain))) {
                certificates.add((X509Certificate) certificate);
            }
        } catch (CertificateException | FileNotFoundException e) {
            throw new IllegalArgumentException("Error Occurred: " + e);
        }
    }

    CertificateKeyPair() {
        this.certificateChain = null;
        this.privateKey = null;
    }

    String certificateChain() {
        return certificateChain;
    }

    String privateKey() {
        return privateKey;
    }

    public SslContext sslContext() {
        return sslContext;
    }

    void setSslContext(SslContext sslContext) {
        this.sslContext = sslContext;
    }

    public byte[] ocspStaplingData() {
        return ocspStaplingData;
    }

    public boolean useOCSPStapling() {
        return useOCSPStapling;
    }

    @Override
    public void run() {
        try {
            OCSPResp response = OCSPClient.response(certificates.get(0), certificates.get(1));
            SingleResp ocspResp = ((BasicOCSPResp) response.getResponseObject()).getResponses()[0];
            if (ocspResp.getCertStatus() == null) {
                ocspStaplingData = response.getEncoded();
                return;
            }
        } catch (Exception ex) {
            // Ignore
        }
        ocspStaplingData = null;
    }

    @Override
    public void close() throws IOException {
        scheduledFuture.cancel(true);
    }
}
