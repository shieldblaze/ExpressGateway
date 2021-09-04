package com.shieldblaze.expressgateway.replica;

import io.netty.buffer.ByteBuf;

import java.util.Objects;

public final class Cluster {

    /**
     * Name of this Cluster
     */
    private String name;

    /**
     * Member just in left side of this Member.
     * <p>
     * (LEFT_MEMBER) - THIS_MEMBER - RIGHT_MEMBER
     */
    private MemberHandler leftMember;

    /**
     * Member just in right side of this Member.
     * <p>
     * LEFT_MEMBER - THIS_MEMBER - (RIGHT_MEMBER)
     */
    private MemberHandler rightMember;

    public String name() {
        return name;
    }

    public void name(String name) {
        this.name = Objects.requireNonNull(name, "Name");
    }

    /**
     * Set left member
     *
     * @param leftMember {@link MemberHandler} instance
     */
    public void leftMember(MemberHandler leftMember) {
        this.leftMember = Objects.requireNonNull(leftMember, "LeftMember");
    }

    /**
     * Set right member
     *
     * @param rightMember {@link MemberHandler} instance
     */
    public void rightMember(MemberHandler rightMember) {
        this.rightMember = Objects.requireNonNull(rightMember, "RightMember");
    }

    /**
     * Send an announcement to the left and right member
     *
     * @param buf {@link ByteBuf} to send
     */
    public void sendAnnouncement(ByteBuf buf) {
        leftMember.sendAnnouncement(buf.retainedDuplicate());
        rightMember.sendAnnouncement(buf.retainedDuplicate());
    }
}
