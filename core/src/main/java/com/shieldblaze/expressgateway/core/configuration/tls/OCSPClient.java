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
package com.shieldblaze.expressgateway.core.configuration.tls;

import org.apache.http.client.config.RequestConfig;
import org.apache.http.client.methods.CloseableHttpResponse;
import org.apache.http.client.methods.HttpPost;
import org.apache.http.entity.ByteArrayEntity;
import org.apache.http.entity.ContentType;
import org.apache.http.impl.client.CloseableHttpClient;
import org.apache.http.impl.client.HttpClientBuilder;
import org.bouncycastle.asn1.ASN1ObjectIdentifier;
import org.bouncycastle.asn1.ASN1Sequence;
import org.bouncycastle.asn1.DEROctetString;
import org.bouncycastle.asn1.DLTaggedObject;
import org.bouncycastle.asn1.ocsp.OCSPObjectIdentifiers;
import org.bouncycastle.asn1.ocsp.OCSPResponseStatus;
import org.bouncycastle.asn1.x509.Extension;
import org.bouncycastle.asn1.x509.Extensions;
import org.bouncycastle.asn1.x509.GeneralName;
import org.bouncycastle.asn1.x509.X509ObjectIdentifiers;
import org.bouncycastle.cert.jcajce.JcaX509CertificateHolder;
import org.bouncycastle.cert.jcajce.JcaX509ExtensionUtils;
import org.bouncycastle.cert.ocsp.BasicOCSPResp;
import org.bouncycastle.cert.ocsp.CertificateID;
import org.bouncycastle.cert.ocsp.OCSPException;
import org.bouncycastle.cert.ocsp.OCSPReq;
import org.bouncycastle.cert.ocsp.OCSPReqBuilder;
import org.bouncycastle.cert.ocsp.OCSPResp;
import org.bouncycastle.operator.ContentVerifierProvider;
import org.bouncycastle.operator.OperatorCreationException;
import org.bouncycastle.operator.jcajce.JcaContentVerifierProviderBuilder;
import org.bouncycastle.operator.jcajce.JcaDigestCalculatorProviderBuilder;

import java.io.IOException;
import java.net.URI;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;
import java.util.Enumeration;

final class OCSPClient {

    private static final String OCSP_REQUEST_TYPE = "application/ocsp-request";
    private static final String OCSP_RESPONSE_TYPE = "application/ocsp-response";

    static OCSPResp getResponse(X509Certificate x509Certificate, X509Certificate issuer) throws Exception {
        CertificateID id = new CertificateID(new JcaDigestCalculatorProviderBuilder().build()
                .get(CertificateID.HASH_SHA1), new JcaX509CertificateHolder(issuer), x509Certificate.getSerialNumber());

        OCSPReqBuilder builder = new OCSPReqBuilder();
        builder.addRequest(id);

        byte[] nonce = new byte[6];
        SecureRandom.getInstanceStrong().nextBytes(nonce);
        DEROctetString derNonce = new DEROctetString(nonce);

        builder.setRequestExtensions(new Extensions(new Extension(OCSPObjectIdentifiers.id_pkix_ocsp_nonce, false, derNonce)));

        OCSPResp ocspResp = queryCA(URI.create(getOcspUrlFromCertificate(x509Certificate)), builder.build());

        if (ocspResp.getStatus() != OCSPResponseStatus.SUCCESSFUL) {
            throw new IllegalArgumentException("OCSP Request was not successful, Status: " + ocspResp.getStatus());
        }

        BasicOCSPResp basicResponse = (BasicOCSPResp) ocspResp.getResponseObject();

        checkNonce(basicResponse, derNonce);
        checkSignature(basicResponse, issuer);

        if (basicResponse.getResponses().length != 1) {
            throw new IllegalArgumentException("Expected number of response was 1 but we got: " + basicResponse.getResponses().length);
        }

        return ocspResp;
    }

    private static OCSPResp queryCA(URI uri, OCSPReq request) throws Exception {
        try (CloseableHttpClient httpClient = HttpClientBuilder.create()
                .setDefaultRequestConfig(RequestConfig.custom()
                        .setRedirectsEnabled(false)
                        .setConnectionRequestTimeout(1000 * 5)
                        .setSocketTimeout(1000 * 5)
                        .setConnectTimeout(1000 * 5).build())
                .build()) {

            HttpPost httpPost = new HttpPost(uri);
            httpPost.setEntity(new ByteArrayEntity(request.getEncoded(), ContentType.create(OCSP_REQUEST_TYPE)));
            httpPost.setHeader("Accept-Content", OCSP_RESPONSE_TYPE);
            httpPost.setHeader("User-Agent", "ShieldBlaze ExpressGateway OCSP Client");

            try (CloseableHttpResponse httpResponse = httpClient.execute(httpPost)) {

                if (!httpResponse.getFirstHeader("Content-Type").getValue().equalsIgnoreCase(OCSP_RESPONSE_TYPE)) {
                    throw new IllegalArgumentException("Response Content-Type was: " +
                            httpResponse.getFirstHeader("Content-Type").getValue() + ", Expected: " + OCSP_RESPONSE_TYPE);
                }

                if (httpResponse.getStatusLine().getStatusCode() != 200) {
                    throw new IllegalArgumentException("HTTP Response Code was: " +
                            httpResponse.getStatusLine().getStatusCode() + ", Expected: 200");
                }

                return new OCSPResp(httpResponse.getEntity().getContent());
            }
        }
    }

    private static String getOcspUrlFromCertificate(X509Certificate cert) throws IOException {
        byte[] extensionValue = cert.getExtensionValue(new ASN1ObjectIdentifier("1.3.6.1.5.5.7.1.1").getId());

        ASN1Sequence asn1Seq = (ASN1Sequence) JcaX509ExtensionUtils.parseExtensionValue(extensionValue);
        Enumeration<?> objects = asn1Seq.getObjects();

        while (objects.hasMoreElements()) {
            ASN1Sequence obj = (ASN1Sequence) objects.nextElement(); // AccessDescription
            ASN1ObjectIdentifier oid = (ASN1ObjectIdentifier) obj.getObjectAt(0); // accessMethod
            DLTaggedObject location = (DLTaggedObject) obj.getObjectAt(1); // accessLocation

            if (location.getTagNo() == GeneralName.uniformResourceIdentifier) {
                DEROctetString uri = (DEROctetString) location.getObject();
                String str = new String(uri.getOctets());
                if (oid.equals(X509ObjectIdentifiers.id_ad_ocsp)) {
                    return str;
                }
            }
        }

        throw new NullPointerException("Unable to find OCSP URL");
    }

    private static void checkNonce(BasicOCSPResp basicResponse, DEROctetString encodedNonce) throws OCSPException {
        Extension nonceExt = basicResponse.getExtension(OCSPObjectIdentifiers.id_pkix_ocsp_nonce);
        if (nonceExt != null) {
            DEROctetString responseNonceString = (DEROctetString) nonceExt.getExtnValue();
            if (!responseNonceString.equals(encodedNonce)) {
                throw new OCSPException("Nonce Mismatch");
            }
        }
    }

    private static void checkSignature(BasicOCSPResp basicResponse, X509Certificate certificate) throws OCSPException {
        try {
            ContentVerifierProvider verifier = new JcaContentVerifierProviderBuilder().build(certificate);
            if (!basicResponse.isSignatureValid(verifier)) {
                throw new OCSPException("OCSP-Signature is not valid!");
            }
        } catch (OperatorCreationException e) {
            throw new OCSPException("Error checking Ocsp-Signature", e);
        }
    }
}
