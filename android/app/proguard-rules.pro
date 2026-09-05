# ML Kit discovers these classes through manifest metadata and invokes their no-argument constructors.
-keep class com.google.mlkit.** implements com.google.firebase.components.ComponentRegistrar {
    public <init>();
}
