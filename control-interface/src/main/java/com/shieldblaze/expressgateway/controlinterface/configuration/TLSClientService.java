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
package com.shieldblaze.expressgateway.controlinterface.configuration;

import com.google.protobuf.ProtocolStringList;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.controlinterface.tls.TLS;
import com.shieldblaze.expressgateway.controlinterface.tls.TLSClientServiceGrpc;
import io.grpc.stub.StreamObserver;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;

import javax.net.ssl.TrustManagerFactory;
import java.io.FileWriter;
import java.security.KeyStore;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Base64;
import java.util.List;
import java.util.stream.Collectors;

public class TLSClientService extends TLSClientServiceGrpc.TLSClientServiceImplBase {

    @Override
    public void client(TLS.Client request, StreamObserver<TLS.ConfigurationResponse> responseObserver) {
        TLS.ConfigurationResponse response;

        try {
            TLSConfiguration tlsConfiguration = TLSConfigurationBuilder.forClient()
                    .withProtocols(protocolConverter(request.getProtocolsList()))
                    .withCiphers(cipherConverter(request.getCiphersList()))
                    .withAcceptAllCertificate(request.getAcceptAllCertificates())
                    .withUseStartTLS(request.getUseStartTLS())
                    .build();

            tlsConfiguration.defaultMapping(new CertificateKeyPair(request.getCertificateChain(), request.getPrivateKey(), false));
            tlsConfiguration.saveTo(request.getProfileName(), request.getPassword());

            response = TLS.ConfigurationResponse.newBuilder()
                    .setResponseCode(1)
                    .setResponseText("Success")
                    .build();
        } catch (Exception ex) {
            response = TLS.ConfigurationResponse.newBuilder()
                    .setResponseCode(-1)
                    .setResponseText("Error: " + ex.getLocalizedMessage())
                    .build();
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(TLS.GetTLS request, StreamObserver<TLS.Client> responseObserver) {
        try {
            TLSConfiguration tlsConfiguration = TLSConfiguration.loadFrom(request.getProfileName(), request.getPassword(), false);
            StringBuilder certificateChain = new StringBuilder();

            for (X509Certificate x509Certificate : tlsConfiguration.defaultMapping().certificates()) {
                certificateChain.append("-----BEGIN CERTIFICATE-----").append("\r\n");
                certificateChain.append(Base64.getEncoder().encodeToString(x509Certificate.getEncoded())).append("\r\n");
                certificateChain.append("-----END CERTIFICATE-----").append("\r\n");
            }

            TLS.Client client = TLS.Client.newBuilder()
                    .setCertificateChain(certificateChain.toString())
                    .setPrivateKey("")
                    .addAllProtocols(tlsConfiguration.protocols().stream().map(Enum::name).collect(Collectors.toList()))
                    .addAllCiphers(tlsConfiguration.ciphers().stream().map(Enum::name).collect(Collectors.toList()))
                    .setUseStartTLS(tlsConfiguration.useStartTLS())
                    .setProfileName(request.getProfileName())
                    .setPassword("")
                    .build();

            responseObserver.onNext(client);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(ex);
            responseObserver.onCompleted();
        }
    }

    private List<Protocol> protocolConverter(ProtocolStringList protocolStringList) {
        List<Protocol> protocols = new ArrayList<>();
        for (String protocol : protocolStringList) {
            protocols.add(Protocol.valueOf(protocol.toUpperCase()));
        }
        return protocols;
    }

    private List<Cipher> cipherConverter(ProtocolStringList protocolStringList) {
        List<Cipher> protocols = new ArrayList<>();
        for (String protocol : protocolStringList) {
            protocols.add(Cipher.valueOf(protocol.toUpperCase()));
        }
        return protocols;
    }
}
