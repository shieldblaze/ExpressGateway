/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import io.netty.handler.ssl.ClientAuth;

public class ServiceDiscoveryContext {

    @JsonIgnore
    private static ServiceDiscoveryContext INSTANCE;

    @JsonProperty
    private String IPAddress;

    @JsonProperty
    private Integer Port;

    @JsonProperty
    private String ZooKeeperConnectionString;

    @JsonProperty
    private Boolean EnableTLS;

    @JsonProperty
    private ClientAuth MTLS;

    @JsonProperty
    private String PKCS12File;

    @JsonProperty
    private String Password;

    @JsonIgnore
    private char[] PasswordAsChars;

    public ServiceDiscoveryContext() {
    }

    /**
     * This should be used in testing only.
     */
    public ServiceDiscoveryContext(String IPAddress, Integer port, String ZooKeeperConnectionString, Boolean enableTLS,
                                   ClientAuth MTLS, String PKCS12File, String password) {
        this.IPAddress = IPAddress;
        Port = port;
        this.ZooKeeperConnectionString = ZooKeeperConnectionString;
        EnableTLS = enableTLS;
        this.MTLS = MTLS;
        this.PKCS12File = PKCS12File;
        Password = password;
    }

    public static void setInstance(ServiceDiscoveryContext expressGateway) {
        INSTANCE = expressGateway;
        INSTANCE.clean();
    }

    public static ServiceDiscoveryContext getInstance() {
        return INSTANCE;
    }

    public String IPAddress() {
        return IPAddress;
    }

    public Integer port() {
        return Port;
    }

    public String zooKeeperConnectionString() {
        return ZooKeeperConnectionString;
    }

    public Boolean enableTLS() {
        return EnableTLS;
    }

    public ClientAuth mTLS() {
        return MTLS;
    }

    public String PKCS12File() {
        return PKCS12File;
    }

    public char[] passwordAsChars() {
        return PasswordAsChars;
    }

    @Override
    public String toString() {
        return "ServiceDiscovery{" +
                "IPAddress='" + IPAddress + '\'' +
                ", Port=" + Port +
                ", ZooKeeperConnectionString=" + ZooKeeperConnectionString +
                ", EnableTLS=" + EnableTLS +
                ", MTLS=" + MTLS +
                ", PKCS12File='" + PKCS12File + '\'' +
                ", Password='*****'}";
    }

    public void clean() {
        PasswordAsChars = Password.toCharArray();
        Password = null;
    }
}
