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
package com.shieldblaze.expressgateway.integration.aws.ec2;

import com.shieldblaze.expressgateway.common.annotation.Async;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.integration.Server;
import com.shieldblaze.expressgateway.integration.aws.ec2.events.EC2ServerDestroyEvent;
import com.shieldblaze.expressgateway.integration.aws.ec2.events.EC2ServerRestartEvent;
import com.shieldblaze.expressgateway.integration.event.ServerDestroyEvent;
import com.shieldblaze.expressgateway.integration.event.ServerRestartEvent;
import software.amazon.awssdk.services.ec2.Ec2Client;
import software.amazon.awssdk.services.ec2.model.Instance;
import software.amazon.awssdk.services.ec2.model.InstanceNetworkInterface;
import software.amazon.awssdk.services.ec2.model.RebootInstancesRequest;
import software.amazon.awssdk.services.ec2.model.RebootInstancesResponse;
import software.amazon.awssdk.services.ec2.model.RunInstancesResponse;
import software.amazon.awssdk.services.ec2.model.Tag;
import software.amazon.awssdk.services.ec2.model.TerminateInstancesRequest;
import software.amazon.awssdk.services.ec2.model.TerminateInstancesResponse;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.util.Objects;

public class EC2Instance implements Server {

    private String id;
    private long startTime;
    private boolean autoscaled;
    private boolean inUse;
    private Inet4Address inet4Address;
    private Inet6Address inet6Address;
    private Ec2Client ec2Client;

    @Override
    public String id() {
        return id;
    }

    @Override
    public long startTime() {
        return startTime;
    }

    @Override
    public boolean autoscaled() {
        return autoscaled;
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
        return inet4Address;
    }

    @Override
    public Inet6Address ipv6Address() {
        return inet6Address;
    }

    @Async
    @Override
    public ServerRestartEvent<RebootInstancesResponse> restart() {
        EC2ServerRestartEvent event = new EC2ServerRestartEvent();

        GlobalExecutors.submitTask(() -> {
            try {
                RebootInstancesResponse rebootInstancesResponse = ec2Client.rebootInstances(RebootInstancesRequest.builder().instanceIds(id).build());
                event.trySuccess(rebootInstancesResponse);
            } catch (Exception ex) {
                event.tryFailure(ex);
            }
        });

        return event;
    }

    @Async
    @Override
    public ServerDestroyEvent<TerminateInstancesResponse> destroy() {
        EC2ServerDestroyEvent event = new EC2ServerDestroyEvent();

        GlobalExecutors.submitTask(() -> {
           try {
               TerminateInstancesResponse terminateInstancesResponse = ec2Client.terminateInstances(TerminateInstancesRequest.builder().instanceIds(id).build());
               event.trySuccess(terminateInstancesResponse);
           } catch (Exception ex) {
               event.tryFailure(ex);
           }
        });

        return event;
    }

    @Override
    public String providerName() {
        return "AWS-EC2";
    }

    @Override
    public String toString() {
        return "EC2Instance{" +
                "id='" + id + '\'' +
                ", startTime=" + startTime +
                ", autoscaled=" + autoscaled +
                ", inUse=" + inUse +
                ", inet4Address=" + inet4Address +
                ", inet6Address=" + inet6Address +
                '}';
    }

    /**
     * Build {@link EC2Instance} Instance from {@link Instance}
     */
    public static EC2Instance buildFrom(Ec2Client ec2Client, Instance instance) {
        Objects.requireNonNull(ec2Client, "EC2Client");
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

        EC2Instance ec2Instance = new EC2Instance();
        ec2Instance.id = instance.instanceId();
        ec2Instance.startTime = instance.launchTime().toEpochMilli();
        ec2Instance.autoscaled = autoscaled;

        try {
            if (instance.publicIpAddress() != null) {
                ec2Instance.inet4Address = (Inet4Address) InetAddress.getByName(instance.publicIpAddress());
            } else {
                ec2Instance.inet4Address = (Inet4Address) InetAddress.getByName(instance.privateIpAddress());
            }

            ec2Instance.inet6Address = (Inet6Address) InetAddress.getByName(instance.networkInterfaces().get(0).ipv6Addresses().get(0).ipv6Address());
        } catch (Exception ex) {
            // This should never happen
        }

        ec2Instance.ec2Client = ec2Client;
        return ec2Instance;
    }
}
