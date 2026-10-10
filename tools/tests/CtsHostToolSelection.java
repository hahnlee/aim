import java.io.File;
import java.util.*;
import com.android.tradefed.config.*;
import com.android.tradefed.testtype.*;
import com.android.tradefed.testtype.suite.*;
import com.android.tradefed.invoker.*;
import com.android.tradefed.invoker.logger.CurrentInvocation;
public class CtsHostToolSelection {
 public static void main(String[] args) throws Exception {
  String module="CtsStagedInstallHostTestCases";
  String option=module+":{config-descriptor}metadata:module-dir-path:="+args[1];
  SuiteModuleLoader loader=new SuiteModuleLoader(new HashMap<>(),new HashMap<>(),List.of(),List.of(option));
  var configs=loader.loadConfigsFromSpecifiedPaths(List.of(new File(args[0])),Set.of(new Abi("arm64-v8a","64")),"cts");
  if(configs.size()!=1) throw new AssertionError(configs.keySet());
  var config=configs.values().iterator().next();
  var paths=config.getConfigurationDescription().getMetaData("module-dir-path");
  System.out.println("metadata="+paths);
  if(!paths.get(0).equals(args[1]))throw new AssertionError(paths);
  InvocationContext context=new InvocationContext();
  context.setConfigurationDescriptor(config.getConfigurationDescription());
  CurrentInvocation.setModuleContext(context);
  TestInformation info=TestInformation.newBuilder().setInvocationContext(context).build();
  File selected=info.getDependencyFile("deapexer.zip",false);
  if(!selected.getCanonicalFile().equals(new File(args[1],"deapexer.zip").getCanonicalFile()))throw new AssertionError(selected);
  System.out.println("selected="+selected.getCanonicalPath());
 }
}
