
> ## ⚠️ Project Archived
> After deep reflection, I have decided to focus my limited time and energy on my other projects. As a solo developer, I have struggled to actively maintain multiple projects with the care and attention they deserve. Without enough sponsors, it became unfeasible to maintain Palmr.
> If you are interested in continuing this work through a fork, I will be happy to add a link to it here in the README.
> We thank all the contributors and users who have supported Palmr over time!


# 🌴 Palmr. - Open-Source File Transfer

<p align="center">
  <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749825361/Group_47_1_bcx8gw.png" alt="Palmr Banner" style="width: 100%;"/>
</p>

**Palmr.** is a **flexible** and **open-source** alternative to file transfer services like **WeTransfer**, **SendGB**, **Send Anywhere**, and **Files.fm**.

<div align="center">
  <div style="background: linear-gradient(135deg, #ff4757, #ff3838); padding: 20px; border-radius: 12px; margin: 20px 0; box-shadow: 0 4px 15px rgba(255, 71, 87, 0.3); border: 2px solid #ff3838;">
    <h3 style="color: white; margin: 0 0 10px 0; font-size: 18px; font-weight: bold;">
      ⚠️ BETA VERSION
    </h3>
    <p style="color: white; margin: 0; font-size: 14px; opacity: 0.95;">
      <strong>This project is currently in beta phase.</strong><br>
      Not recommended for production environments.
    </p>
  </div>
</div>

🔗 **For detailed documentation visit:** [Palmr. - Documentation](https://palmr.kyantech.com.br)

## 📌 Why Choose Palmr.?

- **Self-hosted** – Deploy on your own server or VPS.
- **Full control** – No third-party dependencies, ensuring privacy and security.
- **No artificial limits** – Share files without hidden restrictions or fees.
- **Folder organization** – Create folders to organize and share files.
- **Simple deployment** – SQLite database and filesystem storage for easy setup.
- **Scalable storage** – Optional S3-compatible object storage for enterprise needs.

## 🚀 Technologies Used

### **Palmr.** is built with a focus on **performance**, **scalability**, and **security**.

<div align="center">
  <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1745548231/Palmr./Captura_de_Tela_2025-04-24_a%CC%80s_23.24.26_kr4hsl.png" style="width: 100%; border-radius: 15px;" />
</div>


### **Backend & API**
- **Fastify (Node.js)** – High-performance API framework with built-in schema validation.
- **SQLite** – Lightweight, reliable database with zero-configuration setup.
- **Filesystem Storage** – Direct file storage with optional S3-compatible object storage.

### **Frontend**
- **NextJS 15 + TypeScript + Shadcn/ui** – Modern and fast web interface.


## 🛠️ How It Works

1. **Web Interface** → Built with Next, React and TypeScript for a seamless user experience.
2. **Backend API** → Fastify handles requests and manages file operations.
3. **Database** → SQLite stores metadata and transactional data with zero configuration.
4. **Storage** → Filesystem storage ensures reliable file storage with optional S3-compatible object storage for scalability.

## 📸 Screenshots

<table>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824929/Login_veq6e7.png" alt="Login Page" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Login Page</strong>
    </td>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824929/Home_lzvfzu.png" alt="Home Page" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Home Page</strong>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/Dashboard_uycmxb.png" alt="Dashboard" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Dashboard</strong>
    </td>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824929/Profile_wvnlzw.png" alt="Profile Page" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Profile Page</strong>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/Files_List_ztwr1e.png" alt="Files List View" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Files List View</strong>
    </td>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/Files_Cards_pwsh5e.png" alt="Files Card View" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Files Card View</strong>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824927/Shares_cgplgw.png" alt="Shares Management" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Shares Management</strong>
    </td>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/Reive_Files_uhkeyc.png" alt="Receive Files" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Receive Files</strong>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824927/Default_Reverse_xedmhw.png" alt="Reverse Share" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Reverse Share</strong>
    </td>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/Settings_oampxr.png" alt="Settings Panel" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Settings Panel</strong>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/User_Management_xjbfhn.png" alt="User Management" style="width: 100%; border-radius: 8px;" />
      <br /><strong>User Management</strong>
    </td>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/Forgot_Password_jcz9ad.png" alt="Forgot Password" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Forgot Password</strong>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="https://res.cloudinary.com/technical-intelligence/image/upload/v1749824928/WeTransfer_Reverse_u0g7eb.png" alt="Forgot Password" style="width: 100%; border-radius: 8px;" />
      <br /><strong>Reverse Share (WeTransfer Style)</strong>
    </td>
  </tr>
</table>


## 👨‍💻 Core Maintainers

| [**Daniel Luiz Alves**](https://github.com/danielalves96) |
|------------------|
| <img src="https://github.com/danielalves96.png" width="150px" alt="Daniel Luiz Alves" /> |

</br>

## 🤝 Supporters

[<img src="https://i.ibb.co/nMN40STL/Repoflow.png" width="200px" alt="Daniel Luiz Alves" />](https://www.repoflow.io/)

## ⭐ Star History

  <a href="https://www.star-history.com/#kyantech/Palmr&Date">
   <picture>
     <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=kyantech/Palmr&type=Date&theme=dark" />
     <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=kyantech/Palmr&type=Date" />
     <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=kyantech/Palmr&type=Date" />
   </picture>
  </a>

## 🛠️ Contributing

For contribution guidelines, please refer to the [CONTRIBUTING.md](CONTRIBUTING.md) file.


