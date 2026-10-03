package com.android.server.pm;

public final class UpdateOwnershipOracle {
    private static void snapshot(String name, UpdateOwnershipHelper owner) {
        System.out.print(name);
        for (String target : new String[] {"one", "two", "shared", "missing"})
            System.out.print(" " + owner.isUpdateOwnershipDenylisted(target));
        for (String provider : new String[] {"a", "b", null})
            System.out.print(" " + owner.isUpdateOwnershipDenyListProvider(provider));
        System.out.println();
    }

    public static void main(String[] args) {
        var owner = new UpdateOwnershipHelper();
        owner.addToUpdateOwnerDenyList("a", new android.util.ArraySet<>(new String[] {"one", "shared", "shared"}));
        snapshot("first", owner);
        owner.addToUpdateOwnerDenyList("b", new android.util.ArraySet<>(new String[] {"shared"}));
        snapshot("overlap", owner);
        owner.addToUpdateOwnerDenyList("a", new android.util.ArraySet<>(new String[] {"two"}));
        snapshot("accumulate", owner);
        owner.addToUpdateOwnerDenyList("a", new android.util.ArraySet<>());
        snapshot("empty", owner);
        owner.removeUpdateOwnerDenyList("a");
        snapshot("remove-a", owner);
        owner.removeUpdateOwnerDenyList("a");
        snapshot("repeat-remove", owner);
        owner.removeUpdateOwnerDenyList("b");
        snapshot("remove-last", owner);
    }
}
