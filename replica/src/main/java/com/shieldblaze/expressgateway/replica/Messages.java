package com.shieldblaze.expressgateway.replica;

public final class Messages {

    /**
     * Magic number
     */
    public static final short MAGIC = (short) 0xAAFF;

    /**
     * When a new member is added to the cluster
     */
    public static final byte MEMBER_ADD = (byte) 0xa1;

    /**
     * When an existing member is removed from the cluster
     */
    public static final byte MEMBER_REMOVED = (byte) 0xa2;

    /**
     * When a configuration is being synchronised across cluster
     */
    public static final byte SYNC_CONFIGURATION = (byte) 0xa3;

}
