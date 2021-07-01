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
package com.shieldblaze.expressgateway.integration.aws.lightsail.instance;

import com.shieldblaze.expressgateway.integration.server.Server;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailServerDestroyEvent;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailServerRestartEvent;
import com.shieldblaze.expressgateway.integration.event.ServerDestroyEvent;
import com.shieldblaze.expressgateway.integration.event.ServerRestartEvent;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceRequest;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceResponse;
import software.amazon.awssdk.services.lightsail.model.Instance;
import software.amazon.awssdk.services.lightsail.model.OperationStatus;
import software.amazon.awssdk.services.lightsail.model.RebootInstanceRequest;
import software.amazon.awssdk.services.lightsail.model.RebootInstanceResponse;
import software.amazon.awssdk.services.lightsail.model.Tag;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.Objects;

/**
 * AWS Lightsail Instance
 */
public final class LightsailInstance implements Server {

    private String name;
    private long startTime;
    private boolean autoscaled;
    private boolean inUse;
    private Inet4Address ipv4Address;
    private Inet6Address ipv6Address;
    private LightsailClient lightsailClient;

    @Override
    public String id() {
        return name;
    }

    @Override
    public long startTime() {
        return startTime;
    }

    @Override
    public boolean inUse() {
        return inUse;
    }

    public void inUse(boolean inUse) {
        this.inUse = inUse;
    }

    @Override
    public Inet4Address ipv4Address() {
        return ipv4Address;
    }

    @Override
    public Inet6Address ipv6Address() {
        return ipv6Address;
    }

    @Override
    public ServerRestartEvent<RebootInstanceResponse> restart() {
        LightsailServerRestartEvent event = new LightsailServerRestartEvent();

        try {
            RebootInstanceResponse rebootInstanceResponse = lightsailClient.rebootInstance(RebootInstanceRequest.builder()
                    .instanceName(name)
                    .build());

            if (!rebootInstanceResponse.hasOperations()) {
                throw new IllegalArgumentException("RebootInstanceResponse does not have any Operations");
            }

            OperationStatus status = rebootInstanceResponse.operations().get(0).status();
            if (status == OperationStatus.SUCCEEDED) {
                event.trySuccess(rebootInstanceResponse);
            } else {
                event.tryFailure(new IllegalArgumentException("Instance state: " + status));
            }
        } catch (Exception ex) {
            event.tryFailure(ex);
        }

        return event;
    }

    @Override
    public ServerDestroyEvent<DeleteInstanceResponse> destroy() {
        LightsailServerDestroyEvent event = new LightsailServerDestroyEvent();

        try {
            DeleteInstanceResponse deleteInstanceResponse = lightsailClient.deleteInstance(DeleteInstanceRequest.builder()
                    .instanceName(name)
                    .forceDeleteAddOns(true)
                    .build());

            if (!deleteInstanceResponse.hasOperations()) {
                throw new IllegalArgumentException("DeleteInstanceResponse does not have any Operations");
            }

            OperationStatus status = deleteInstanceResponse.operations().get(0).status();
            if (status == OperationStatus.SUCCEEDED) {
                event.trySuccess(deleteInstanceResponse);
            } else {
                event.tryFailure(new IllegalArgumentException("Instance state: " + status));
            }
        } catch (Exception ex) {
            event.tryFailure(ex);
        }

        return event;
    }

    @Override
    public String providerName() {
        return "AWS-LIGHTSAIL";
    }

    @Override
    public String toString() {
        return "LightsailServer{" +
                "name='" + name + '\'' +
                ", startTime=" + startTime +
                ", autoscaled=" + autoscaled +
                ", inUse=" + inUse +
                ", ipv4Address=" + ipv4Address +
                ", ipv6Address=" + ipv6Address +
                '}';
    }

    /**
     * Build {@link LightsailInstance} from {@link Instance}
     *
     * @throws IllegalArgumentException If {@link Instance} is not valid.
     * @throws UnknownHostException     If IP address is not valid.
     */
    public static LightsailInstance buildFrom(LightsailClient lightsailClient, Instance instance) {
        Objects.requireNonNull(lightsailClient, "LightsailClient");
        Objects.requireNonNull(instance, "Instance");

        if (!instance.hasTags()) {
            throw new IllegalArgumentException("Instance does not have any tags");
        }

        boolean foundTag = false;
        boolean autoscaled = false;
        for (Tag tag : instance.tags()) {
            if (tag.key().equalsIgnoreCase("ExpressGateway")) {
                if (tag.value().equalsIgnoreCase("Master")) {
                    foundTag = true;
                    break;
                } else if (tag.value().equalsIgnoreCase("Autoscaled")) {
                    autoscaled = true;
                    foundTag = true;
                    break;
                }
            }
        }

        if (!foundTag) {
            throw new IllegalArgumentException("Instance does not have valid tags");
        }

        InetAddress ipv4 = null;
        InetAddress ipv6 = null;
        try {
            if (instance.publicIpAddress() != null) {
                ipv4 = InetAddress.getByName(instance.publicIpAddress());
            } else {
                ipv4 = InetAddress.getByName(instance.privateIpAddress());
            }

            /*
             * See bug report: https://github.com/aws/aws-sdk-java-v2/issues/2515
             */
            if (instance.hasIpv6Addresses() && instance.ipv6Addresses().size() != 0) {
                ipv6 = InetAddress.getByName(instance.ipv6Addresses().get(0));
            }
        } catch (Exception ex) {
            // This can never happen
        }

        LightsailInstance lightsailInstance = new LightsailInstance();
        lightsailInstance.name = instance.name();
        lightsailInstance.startTime = instance.createdAt().toEpochMilli();
        lightsailInstance.autoscaled = autoscaled;
        lightsailInstance.ipv4Address = (Inet4Address) ipv4;
        lightsailInstance.ipv6Address = (Inet6Address) ipv6;
        lightsailInstance.lightsailClient = lightsailClient;

        return lightsailInstance;
    }
}
